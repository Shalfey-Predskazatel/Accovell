use crate::{
    commands,
    core::AppCore,
    model::{AccountView, AppSettings, CodexPreferences, DashboardData, PromptView, UsageWindow},
    sound,
};
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Layout, RichText, Stroke, Vec2};
use std::{
    collections::HashMap,
    future::Future,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tokio::runtime::Handle;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

const BACKGROUND: Color32 = Color32::from_rgb(11, 20, 22);
const SURFACE: Color32 = Color32::from_rgb(20, 33, 36);
const SURFACE_HI: Color32 = Color32::from_rgb(28, 45, 47);
const BORDER: Color32 = Color32::from_rgb(53, 75, 72);
const TEXT: Color32 = Color32::from_rgb(237, 242, 237);
const MUTED: Color32 = Color32::from_rgb(154, 174, 166);
const ACCENT: Color32 = Color32::from_rgb(155, 219, 202);
const BLUE: Color32 = Color32::from_rgb(132, 183, 204);
const GOLD: Color32 = Color32::from_rgb(232, 190, 121);
const RED: Color32 = Color32::from_rgb(229, 143, 129);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Settings,
    Codex,
    Backup,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmAction {
    Exit,
    ResetSettings,
    ResetApplication,
    RemoveAccount,
}

#[derive(Clone, Copy)]
enum HeaderIcon {
    Refresh,
    Backup,
    Codex,
    Settings,
    SoundOn,
    SoundOff,
    Trash,
    Lock,
}

#[derive(Clone, Copy)]
enum WindowTransition {
    Hide(Instant),
    Show(Instant),
    Close(Instant),
}

const WINDOW_TRANSITION_DURATION: Duration = Duration::from_millis(190);

struct Toast {
    title: String,
    detail: String,
    error: bool,
    at: Instant,
}

enum UiMessage {
    Dashboard(Result<DashboardData, String>),
    TrayAction(&'static str),
    TrayExitSucceeded,
    TrayExitFailed(String),
    Action {
        name: String,
        result: Result<Option<DashboardData>, String>,
    },
}

pub struct NativeApp {
    core: Arc<AppCore>,
    runtime: Handle,
    context: egui::Context,
    transition_layer_id: egui::Id,
    sender: mpsc::Sender<UiMessage>,
    receiver: mpsc::Receiver<UiMessage>,
    dashboard: Option<DashboardData>,
    busy: Option<String>,
    toast: Option<Toast>,
    toast_events: Vec<(String, String, bool, i64)>,
    panel: Option<Panel>,
    confirmation: Option<ConfirmAction>,
    confirmation_step: u8,
    pending_remove: Option<(String, String)>,
    settings: AppSettings,
    preferences: CodexPreferences,
    prompts: Vec<PromptView>,
    selected_prompt: usize,
    new_prompt: bool,
    prompt_name: String,
    prompt_content: String,
    admin_password: String,
    current_password: String,
    new_password: String,
    confirm_password: String,
    backup_password: String,
    interval_text: String,
    interval_picker: IntervalPickerState,
    sound_enabled: bool,
    last_sync: Instant,
    window_transition: Option<WindowTransition>,
    was_minimized: bool,
    was_maximized: bool,
    exiting: bool,
    active_change_pending: bool,
    tray: Option<TrayIcon>,
    tray_error: Option<String>,
    reorder_animations: HashMap<String, Instant>,
}

impl NativeApp {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        core: Arc<AppCore>,
        runtime: Handle,
        tray: Result<TrayIcon, String>,
        start_hidden: bool,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (tray, tray_error) = match tray {
            Ok(tray) => (Some(tray), None),
            Err(error) => (None, Some(error)),
        };
        let app = Self {
            core: core.clone(),
            runtime: runtime.clone(),
            context: context.egui_ctx.clone(),
            transition_layer_id: egui::Id::unique("window-transition"),
            sender,
            receiver,
            dashboard: None,
            busy: None,
            toast: None,
            toast_events: Vec::new(),
            panel: None,
            confirmation: None,
            confirmation_step: 0,
            pending_remove: None,
            settings: AppSettings::default(),
            preferences: CodexPreferences::default(),
            prompts: Vec::new(),
            selected_prompt: 0,
            new_prompt: false,
            prompt_name: String::new(),
            prompt_content: String::new(),
            admin_password: String::new(),
            current_password: String::new(),
            new_password: String::new(),
            confirm_password: String::new(),
            backup_password: String::new(),
            interval_text: "60".to_owned(),
            interval_picker: IntervalPickerState::default(),
            sound_enabled: true,
            last_sync: Instant::now(),
            window_transition: (!start_hidden).then_some(WindowTransition::Show(Instant::now())),
            was_minimized: false,
            was_maximized: false,
            exiting: false,
            active_change_pending: false,
            tray,
            tray_error,
            reorder_animations: HashMap::new(),
        };
        app.install_tray_events();
        app.watch_dashboard();
        let sender = app.sender.clone();
        let core = core.clone();
        let repaint_context = app.context.clone();
        runtime.spawn(async move {
            let _ = sender.send(UiMessage::Dashboard(commands::load_dashboard(&core).await));
            repaint_context.request_repaint();
        });
        app.apply_theme(&context.egui_ctx);
        app
    }

    fn watch_dashboard(&self) {
        let mut changes = self.core.subscribe_dashboard_changes();
        let mut shutdown = self.core.subscribe_shutdown();
        let sender = self.sender.clone();
        let core = self.core.clone();
        let context = self.context.clone();
        self.runtime.spawn(async move {
            let _ = changes.borrow_and_update();
            loop {
                tokio::select! {
                    changed = changes.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        let result = commands::load_dashboard(&core).await;
                        if sender.send(UiMessage::Dashboard(result)).is_err() {
                            return;
                        }
                        context.request_repaint();
                    }
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return;
                        }
                    }
                }
            }
        });
    }

    fn install_tray_events(&self) {
        let context = self.context.clone();
        let sender = self.sender.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                let _ = sender.send(UiMessage::TrayAction("show"));
                context.request_repaint();
            }
        }));
        let context = self.context.clone();
        let sender = self.sender.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.0.as_str() {
            "show" => {
                let _ = sender.send(UiMessage::TrayAction("show"));
                context.request_repaint();
            }
            "refresh" => {
                let _ = sender.send(UiMessage::TrayAction("show"));
                let _ = sender.send(UiMessage::TrayAction("refresh"));
                context.request_repaint();
            }
            "quit" => {
                let _ = sender.send(UiMessage::TrayAction("quit"));
                context.request_repaint();
            }
            _ => {}
        }));
    }

    fn apply_theme(&self, context: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.dark_mode = true;
        visuals.panel_fill = BACKGROUND;
        visuals.window_fill = SURFACE;
        visuals.extreme_bg_color = Color32::from_rgb(8, 15, 17);
        visuals.faint_bg_color = SURFACE;
        visuals.override_text_color = Some(TEXT);
        visuals.selection.bg_fill = ACCENT.gamma_multiply(0.28);
        visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        visuals.widgets.noninteractive.bg_fill = SURFACE;
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        visuals.widgets.inactive.bg_fill = SURFACE_HI;
        visuals.widgets.inactive.weak_bg_fill = SURFACE;
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, MUTED);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(38, 61, 59);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        visuals.widgets.active.bg_fill = Color32::from_rgb(49, 80, 73);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
        visuals.widgets.open.bg_fill = SURFACE_HI;
        visuals.window_corner_radius = CornerRadius::same(16);
        visuals.menu_corner_radius = CornerRadius::same(10);
        visuals.widgets.inactive.corner_radius = CornerRadius::same(9);
        visuals.widgets.hovered.corner_radius = CornerRadius::same(9);
        visuals.widgets.active.corner_radius = CornerRadius::same(9);
        visuals.widgets.noninteractive.corner_radius = CornerRadius::same(9);
        visuals.window_shadow = egui::Shadow {
            offset: [0, 12],
            blur: 30,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        context.set_visuals(visuals);
        let mut style = (*context.style_of(egui::Theme::Dark)).clone();
        style.spacing.item_spacing = Vec2::new(11.0, 10.0);
        style.spacing.button_padding = Vec2::new(14.0, 9.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(25.0));
        context.set_style_of(egui::Theme::Dark, style);
    }

    fn poll_messages(&mut self) {
        let window_hidden = self.context.input(|input| {
            let viewport = input.viewport();
            viewport.visible() == Some(false) || viewport.minimized == Some(true)
        });
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                UiMessage::Dashboard(result) => match result {
                    Ok(data) => self.apply_dashboard(data, None),
                    Err(error) => self.notify("Не удалось обновить данные", error, true),
                },
                UiMessage::TrayAction(action) => match action {
                    "show" => {
                        self.context
                            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        self.context.send_viewport_cmd(egui::ViewportCommand::Focus);
                        self.window_transition = Some(WindowTransition::Show(Instant::now()));
                        self.play("restore");
                        self.context.request_repaint();
                    }
                    "refresh" => self.refresh(),
                    "quit" => self.quit_from_tray(),
                    _ => {}
                },
                UiMessage::TrayExitSucceeded => {
                    self.exiting = true;
                    if window_hidden {
                        // A hidden viewport does not run normal egui UI passes, so
                        // it cannot paint the exit animation. Close it directly.
                        self.context.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        self.window_transition = Some(WindowTransition::Close(Instant::now()));
                        self.context.request_repaint();
                    }
                }
                UiMessage::TrayExitFailed(error) => {
                    self.exiting = false;
                    self.context
                        .send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    self.context.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.notify("Не удалось завершить программу", error, true);
                }
                UiMessage::Action { name, result } => {
                    self.busy = None;
                    self.active_change_pending = false;
                    match result {
                        Ok(data) => {
                            if let Some(data) = data {
                                self.apply_dashboard(data, Some(&name));
                            }
                            self.notify_with_sound(
                                &format!("{name}: готово"),
                                "Данные сохранены.",
                                false,
                                completion_sound(&name),
                            );
                            if self.exiting && name == "Выход" {
                                self.core.lock_admin();
                                self.confirmation = None;
                                self.window_transition =
                                    Some(WindowTransition::Close(Instant::now()));
                            }
                        }
                        Err(error) => {
                            self.exiting = false;
                            self.notify(&format!("Не удалось выполнить: {name}"), error, true);
                            if name == "Выход" {
                                self.confirmation = Some(ConfirmAction::Exit);
                            }
                        }
                    }
                }
            }
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| toast.at.elapsed() > Duration::from_secs(6))
        {
            self.toast = None;
        }
    }

    fn apply_dashboard(&mut self, data: DashboardData, action_name: Option<&str>) {
        let previous_active = self
            .dashboard
            .as_ref()
            .and_then(|previous| previous.current_account_id.as_deref());
        let active_changed =
            previous_active.is_some() && previous_active != data.current_account_id.as_deref();
        let auto_switched_account = (active_changed
            && !self.active_change_pending
            && !action_changes_active_account(action_name))
        .then(|| {
            data.accounts
                .iter()
                .find(|account| Some(account.id.as_str()) == data.current_account_id.as_deref())
                .map(|account| account.email.clone())
        })
        .flatten();
        let changed_order = self
            .dashboard
            .as_ref()
            .map(|previous| {
                let old_positions = previous
                    .accounts
                    .iter()
                    .enumerate()
                    .map(|(index, account)| (account.id.as_str(), index))
                    .collect::<HashMap<_, _>>();
                data.accounts
                    .iter()
                    .enumerate()
                    .filter(|(index, account)| {
                        old_positions
                            .get(account.id.as_str())
                            .is_some_and(|old_index| old_index != index)
                    })
                    .map(|(_, account)| account.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let order_changed_at = Instant::now();
        for account_id in changed_order {
            self.reorder_animations.insert(account_id, order_changed_at);
        }
        let was_locked = self
            .dashboard
            .as_ref()
            .is_some_and(|old| old.admin_protected && !old.admin_unlocked);
        let is_locked = data.admin_protected && !data.admin_unlocked;
        if is_locked && !was_locked {
            self.admin_password.clear();
            self.panel = None;
            self.core.lock_admin();
        }
        self.sound_enabled = data.settings.sound_enabled;
        if self.panel != Some(Panel::Settings) {
            self.settings = data.settings.clone();
            self.interval_text = data.settings.refresh_interval_seconds.to_string();
        }
        if self.panel != Some(Panel::Codex) {
            self.preferences = data.codex_preferences.clone();
        }
        self.prompts = data.prompts.clone();
        if !self.new_prompt && self.panel != Some(Panel::Codex) {
            self.selected_prompt = self
                .selected_prompt
                .min(self.prompts.len().saturating_sub(1));
            if let Some(prompt) = self.prompts.get(self.selected_prompt) {
                self.prompt_name = prompt.name.clone();
                self.prompt_content = prompt.content.clone();
            }
        }
        if let Some(error) = data.proxy_error.as_deref() {
            if self.last_sync.elapsed() > Duration::from_secs(20) {
                self.notify("Локальный маршрут Codex недоступен", error, true);
            }
        }
        if let Some(error) = self.tray_error.take() {
            self.notify(
                "Системный трей недоступен",
                format!("Окно оставлено открытым. {error}"),
                true,
            );
        }
        self.last_sync = Instant::now();
        self.dashboard = Some(data);
        if let Some(email) = auto_switched_account {
            self.notify_with_sound(
                "Аккаунт переключён автоматически",
                format!("Codex продолжит работу с аккаунтом {email}."),
                false,
                "auto_switch_complete",
            );
        }
    }

    fn notify(&mut self, title: &str, detail: impl Into<String>, error: bool) {
        self.notify_with_sound(title, detail, error, feedback_sound(title, error));
    }

    fn notify_with_sound(
        &mut self,
        title: &str,
        detail: impl Into<String>,
        error: bool,
        sound_name: &str,
    ) {
        let detail = detail.into();
        self.toast = Some(Toast {
            title: title.to_owned(),
            detail: detail.clone(),
            error,
            at: Instant::now(),
        });
        self.toast_events.insert(
            0,
            (
                title.to_owned(),
                detail,
                error,
                chrono::Utc::now().timestamp(),
            ),
        );
        self.toast_events.truncate(8);
        sound::play(sound_name, self.sound_enabled);
    }

    fn play(&self, name: &str) {
        sound::play(name, self.sound_enabled);
    }

    fn start_action<F>(&mut self, name: &str, future: F)
    where
        F: Future<Output = Result<Option<DashboardData>, String>> + Send + 'static,
    {
        if self.busy.is_some() {
            return;
        }
        self.active_change_pending |= action_changes_active_account(Some(name));
        self.busy = Some(name.to_owned());
        let sender = self.sender.clone();
        let context = self.context.clone();
        let name = name.to_owned();
        self.runtime.spawn(async move {
            let result = future.await;
            let _ = sender.send(UiMessage::Action { name, result });
            context.request_repaint();
        });
    }

    fn start_dashboard_action<F>(&mut self, name: &str, future: F)
    where
        F: Future<Output = Result<DashboardData, String>> + Send + 'static,
    {
        self.start_action(name, async move { future.await.map(Some) });
    }

    fn quit_from_tray(&mut self) {
        if self.exiting {
            return;
        }
        if self.busy.is_some() {
            self.context
                .send_viewport_cmd(egui::ViewportCommand::Visible(true));
            self.context.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.notify(
                "Подождите завершения действия",
                "Закройте приложение из трея ещё раз после завершения текущей операции.",
                false,
            );
            return;
        }

        self.exiting = true;
        let core = self.core.clone();
        let context = self.context.clone();
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            match commands::shutdown(&core).await {
                Ok(()) => {
                    let _ = sender.send(UiMessage::TrayExitSucceeded);
                    context.request_repaint();
                }
                Err(error) => {
                    let _ = sender.send(UiMessage::TrayExitFailed(error));
                    context.request_repaint();
                }
            }
        });
    }

    fn refresh(&mut self) {
        let core = self.core.clone();
        self.play("refresh");
        self.start_dashboard_action("Обновление лимитов", async move {
            commands::refresh_all(&core).await
        });
    }

    fn open_panel(&mut self, panel: Panel) {
        if self
            .dashboard
            .as_ref()
            .is_some_and(|data| data.admin_protected && !data.admin_unlocked)
        {
            self.notify(
                "Доступ закрыт",
                "Введите пароль администратора, чтобы открыть данные.",
                true,
            );
            return;
        }
        self.panel = Some(panel);
        self.play(match panel {
            Panel::Settings => "settings",
            Panel::Codex => "codex",
            Panel::Backup => "backup",
        });
    }

    fn update_settings(&mut self) {
        let Ok(seconds) = self.interval_text.trim().parse::<u32>() else {
            self.notify(
                "Неверная частота проверки",
                "Введите целое число от 1 до 86400 секунд.",
                true,
            );
            return;
        };
        if !(1..=86_400).contains(&seconds) {
            self.notify(
                "Неверная частота проверки",
                "Допустимый интервал — от 1 секунды до 24 часов.",
                true,
            );
            return;
        }
        self.settings.refresh_interval_seconds = seconds;
        self.settings.proxy_enabled = self
            .dashboard
            .as_ref()
            .is_some_and(|data| data.settings.proxy_enabled);
        let core = self.core.clone();
        let settings = self.settings.clone();
        self.play("settings");
        self.start_dashboard_action("Сохранение настроек", async move {
            commands::save_settings(&core, settings).await
        });
    }

    fn render_interval_picker(&mut self, ui: &mut egui::Ui) {
        let current = self
            .interval_text
            .trim()
            .parse::<u32>()
            .unwrap_or(self.settings.refresh_interval_seconds)
            .clamp(1, 86_400);
        if let Some((seconds, unit, direction)) =
            draw_interval_picker(ui, current, &mut self.interval_picker)
        {
            self.interval_text = seconds.to_string();
            self.play(interval_sound(unit, direction));
        }
    }

    fn begin_confirmation(&mut self, action: ConfirmAction) {
        self.confirmation = Some(action);
        self.confirmation_step = 0;
        self.play(match action {
            ConfirmAction::Exit => "exit_prompt",
            ConfirmAction::ResetSettings => "settings_reset_prompt",
            ConfirmAction::ResetApplication => "app_reset_prompt",
            ConfirmAction::RemoveAccount => "account_remove_prompt",
        });
    }

    fn advance_confirmation(&mut self) {
        let Some(action) = self.confirmation else {
            return;
        };
        let required = match action {
            ConfirmAction::Exit => 1,
            ConfirmAction::ResetSettings => 2,
            ConfirmAction::ResetApplication => 3,
            ConfirmAction::RemoveAccount => 1,
        };
        self.confirmation_step += 1;
        if self.confirmation_step < required {
            self.play(match action {
                ConfirmAction::ResetSettings => "settings_confirm_step",
                ConfirmAction::ResetApplication => "app_reset_confirm_step",
                ConfirmAction::Exit | ConfirmAction::RemoveAccount => "warning",
            });
            return;
        }
        self.confirmation = None;
        match action {
            ConfirmAction::Exit => {
                let core = self.core.clone();
                self.exiting = true;
                self.play("exit_confirm");
                self.start_action("Выход", async move {
                    commands::shutdown(&core).await.map(|()| None)
                });
            }
            ConfirmAction::ResetSettings => {
                let core = self.core.clone();
                self.play("settings_reset_confirm");
                self.start_dashboard_action("Сброс настроек", async move {
                    commands::reset_settings_to_default(&core).await
                });
            }
            ConfirmAction::ResetApplication => {
                let core = self.core.clone();
                self.play("app_reset_confirm");
                self.start_dashboard_action("Полный сброс", async move {
                    commands::reset_application(&core).await
                });
            }
            ConfirmAction::RemoveAccount => {
                if let Some((id, email)) = self.pending_remove.take() {
                    let core = self.core.clone();
                    self.play("account_remove_confirm");
                    self.start_dashboard_action(&format!("Удаление {email}"), async move {
                        commands::remove_account(&core, id).await
                    });
                }
            }
        }
    }

    fn render_header(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.brand_mark(ui, 38.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Codex Accounts")
                                .strong()
                                .size(18.0)
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new("АККАУНТЫ  ·  ЛИМИТЫ  ·  НАСТРОЙКИ CODEX")
                                .size(9.5)
                                .color(MUTED)
                                .monospace(),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let sound_icon = if self.sound_enabled {
                            HeaderIcon::SoundOn
                        } else {
                            HeaderIcon::SoundOff
                        };
                        if self.header_button(
                            ui,
                            sound_icon,
                            if self.sound_enabled {
                                "Выключить звуковые эффекты"
                            } else {
                                "Включить звуковые эффекты"
                            },
                        ) {
                            self.sound_enabled = !self.sound_enabled;
                            self.settings.sound_enabled = self.sound_enabled;
                            sound::play(
                                if self.sound_enabled {
                                    "sound_on"
                                } else {
                                    "sound_off"
                                },
                                true,
                            );
                            let core = self.core.clone();
                            let settings = self.settings.clone();
                            self.start_dashboard_action("Звуковые эффекты", async move {
                                commands::save_settings(&core, settings).await
                            });
                        }
                        if self.header_button(ui, HeaderIcon::Settings, "Настройки приложения")
                        {
                            self.open_panel(Panel::Settings);
                        }
                        if self.header_button(ui, HeaderIcon::Backup, "Резервная копия и экспорт")
                        {
                            self.open_panel(Panel::Backup);
                        }
                        if self.header_button(ui, HeaderIcon::Codex, "Настройки Codex") {
                            self.open_panel(Panel::Codex);
                        }
                        if self.header_button(ui, HeaderIcon::Refresh, "Обновить все лимиты")
                        {
                            self.refresh();
                        }
                    });
                });
            });
        ui.add_space(13.0);
    }

    fn brand_mark(&self, ui: &mut egui::Ui, size: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(13), Color32::from_rgb(28, 51, 50));
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(13),
            Stroke::new(1.0, ACCENT.gamma_multiply(0.48)),
            egui::StrokeKind::Inside,
        );
        let center = rect.center();
        ui.painter().text(
            center + Vec2::new(-size * 0.015, -size * 0.015),
            egui::Align2::CENTER_CENTER,
            "C",
            FontId::proportional(size * 0.68),
            TEXT,
        );
        ui.painter().circle_filled(
            center + Vec2::new(size * 0.32, -size * 0.28),
            size * 0.065,
            BLUE,
        );
    }

    fn header_button(&self, ui: &mut egui::Ui, icon: HeaderIcon, hint: &str) -> bool {
        let locked = self
            .dashboard
            .as_ref()
            .is_some_and(|data| data.admin_protected && !data.admin_unlocked);
        let response = ui.add_enabled(
            self.busy.is_none() && !locked,
            egui::Button::new("")
                .min_size(Vec2::splat(42.0))
                .fill(SURFACE_HI)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(12),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), hint)
        });
        let color = if response.hovered() || response.is_pointer_button_down_on() {
            ACCENT
        } else {
            TEXT
        };
        paint_header_icon(ui.painter(), response.rect.shrink(10.0), icon, color);
        response.on_hover_text(hint).clicked()
    }

    fn render_dashboard(&mut self, ui: &mut egui::Ui) {
        let locked = self
            .dashboard
            .as_ref()
            .is_some_and(|data| data.admin_protected && !data.admin_unlocked);
        if locked {
            self.render_lock_screen(ui);
            return;
        }
        let Some(data) = self.dashboard.clone() else {
            self.render_startup(ui);
            return;
        };
        let width = ui.available_width();
        let height = ui.available_height();
        let left_width = (width * 0.62 - 6.0).max(450.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(left_width, height),
                Layout::top_down(Align::Min),
                |ui| {
                    self.render_overview(ui, &data);
                    ui.add_space(12.0);
                    self.render_accounts(ui, &data);
                },
            );
            ui.allocate_ui_with_layout(
                Vec2::new((width - left_width - 12.0).max(320.0), height),
                Layout::top_down(Align::Min),
                |ui| self.render_summary(ui, &data),
            );
        });
    }

    fn render_overview(&self, ui: &mut egui::Ui, data: &DashboardData) {
        let ready = data
            .accounts
            .iter()
            .filter(|account| {
                account_switch_remaining(account)
                    .is_some_and(|remaining| remaining > i32::from(data.settings.switch_threshold))
            })
            .count();
        let best = data.accounts.iter().filter_map(account_remaining).max();
        egui::Frame::new()
            .fill(Color32::from_rgb(27, 45, 45))
            .stroke(Stroke::new(1.0, Color32::from_rgb(61, 91, 82)))
            .corner_radius(18)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.set_min_height(135.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("ПУЛ АККАУНТОВ")
                                .strong()
                                .size(9.0)
                                .color(ACCENT)
                                .monospace(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let (dot_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(7.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot_rect.center(), 3.0, BLUE);
                            ui.label(
                                RichText::new("ПРИОРИТЕТ: СВОБОДНЫЙ ЛИМИТ")
                                    .size(8.0)
                                    .color(MUTED)
                                    .monospace(),
                            );
                        });
                    });
                    ui.add_space(3.0);
                    ui.label(
                        RichText::new("Доступный запас лимита")
                            .strong()
                            .size(21.0)
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new("Больше остаток — выше; при равенстве — ранний сброс.")
                            .size(10.0)
                            .color(MUTED),
                    );
                    ui.add_space(11.0);
                    ui.columns(3, |columns| {
                        overview_stat(
                            &mut columns[0],
                            "ГОТОВЫ",
                            format!("{ready} / {}", data.accounts.len()),
                            BLUE,
                        );
                        overview_stat(
                            &mut columns[1],
                            "ЛУЧШИЙ ОСТАТОК",
                            best.map(|value| format!("{value}%"))
                                .unwrap_or_else(|| "—".to_owned()),
                            best.map(quota_color).unwrap_or(MUTED),
                        );
                        overview_stat(
                            &mut columns[2],
                            "ОБНОВЛЕНО",
                            data.refreshed_at
                                .map(format_updated)
                                .unwrap_or_else(|| "ожидается".to_owned()),
                            MUTED,
                        );
                    });
                });
            });
    }

    fn render_startup(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(105.0);
            self.brand_mark(ui, 74.0);
            ui.add_space(18.0);
            ui.heading("Готовим рабочее пространство");
            ui.label(
                RichText::new("Читаем локальный список аккаунтов и проверяем маршрут Codex.")
                    .color(MUTED),
            );
            ui.add_space(15.0);
            ui.spinner();
            if let Some(toast) = &self.toast {
                ui.add_space(14.0);
                ui.label(RichText::new(&toast.detail).color(if toast.error {
                    RED
                } else {
                    ACCENT
                }));
            }
        });
    }

    fn render_lock_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(95.0);
            self.brand_mark(ui, 64.0);
            ui.add_space(15.0);
            ui.heading("Рабочее пространство защищено");
            ui.label(
                RichText::new("Введите пароль администратора для просмотра аккаунтов и настроек.")
                    .color(MUTED),
            );
            ui.add_space(13.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.admin_password)
                    .password(true)
                    .hint_text("Пароль администратора")
                    .desired_width(320.0),
            );
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                self.unlock_admin();
            }
            ui.add_space(8.0);
            if self.primary_button(ui, "Открыть доступ") {
                self.unlock_admin();
            }
        });
    }

    fn unlock_admin(&mut self) {
        if self.admin_password.is_empty() {
            self.notify("Введите пароль", "Поле пароля администратора пустое.", true);
            return;
        }
        let password = std::mem::take(&mut self.admin_password);
        let core = self.core.clone();
        self.start_dashboard_action("Проверка пароля", async move {
            commands::unlock_admin(&core, password).await
        });
    }

    fn render_accounts(&mut self, ui: &mut egui::Ui, data: &DashboardData) {
        let panel_height = ui.available_height();
        let remaining_ready = data
            .accounts
            .iter()
            .filter(|account| {
                account_switch_remaining(account)
                    .is_some_and(|value| value > i32::from(data.settings.switch_threshold))
            })
            .count();
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(16, 13))
            .show(ui, |ui| {
                ui.set_min_height(panel_height);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("АККАУНТЫ CODEX")
                                .size(10.0)
                                .strong()
                                .color(MUTED)
                                .monospace(),
                        );
                        ui.horizontal(|ui| {
                            ui.heading("Ваши аккаунты");
                            ui.label(
                                RichText::new(format!(
                                    "{} профилей  ·  {} готовы",
                                    data.accounts.len(),
                                    remaining_ready
                                ))
                                .color(MUTED)
                                .size(10.0),
                            );
                        });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.primary_button(ui, "+  Добавить аккаунт") {
                            let core = self.core.clone();
                            self.play("add");
                            self.start_dashboard_action("Добавление аккаунта", async move {
                                commands::add_account(&core).await
                            });
                        }
                    });
                });
                ui.add_space(9.0);
                if data.accounts.is_empty() {
                    ui.add_space((ui.available_height() * 0.30).clamp(72.0, 180.0));
                    ui.vertical_centered(|ui| {
                        self.brand_mark(ui, 52.0);
                        ui.label(
                            RichText::new("Здесь появятся ваши аккаунты")
                                .strong()
                                .size(15.0),
                        );
                        ui.label(
                            RichText::new(
                                "Войдите через официальный экран Codex, чтобы сохранить профиль.",
                            )
                            .color(MUTED),
                        );
                    });
                    ui.add_space(32.0);
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - 62.0)
                    .show(ui, |ui| {
                        for (index, account) in data.accounts.iter().enumerate() {
                            self.render_account_card(ui, account, index);
                            ui.add_space(8.0);
                        }
                    });
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(
                            "Сначала больше свободного лимита · при равенстве — раньше сброс",
                        )
                        .color(MUTED)
                        .size(10.0),
                    );
                });
            });
    }

    fn render_account_card(&mut self, ui: &mut egui::Ui, account: &AccountView, index: usize) {
        let is_active = account.is_active;
        let response = egui::Frame::new()
            .fill(if is_active {
                Color32::from_rgb(27, 48, 45)
            } else {
                Color32::from_rgb(17, 29, 31)
            })
            .stroke(Stroke::new(
                if is_active { 1.1 } else { 0.8 },
                if is_active {
                    ACCENT.gamma_multiply(0.5)
                } else {
                    BORDER.gamma_multiply(0.8)
                },
            ))
            .corner_radius(15)
            .inner_margin(egui::Margin::symmetric(15, 13))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let hue = [ACCENT, GOLD, BLUE, Color32::from_rgb(193, 151, 166)][index % 4];
                    let (avatar, _) =
                        ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        avatar,
                        CornerRadius::same(11),
                        hue.gamma_multiply(0.19),
                    );
                    ui.painter().rect_stroke(
                        avatar,
                        CornerRadius::same(11),
                        Stroke::new(1.0, hue.gamma_multiply(0.35)),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        avatar.center(),
                        egui::Align2::CENTER_CENTER,
                        account
                            .email
                            .chars()
                            .next()
                            .unwrap_or('?')
                            .to_uppercase()
                            .to_string(),
                        FontId::proportional(15.0),
                        hue,
                    );
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&account.email)
                                    .strong()
                                    .size(13.0)
                                    .color(TEXT),
                            );
                            if is_active {
                                ui.label(
                                    RichText::new("● ТЕКУЩИЙ")
                                        .size(9.0)
                                        .color(ACCENT)
                                        .monospace(),
                                );
                            }
                        });
                        ui.label(RichText::new(&account.plan).color(MUTED).size(10.0));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let switch_button = if is_active {
                            egui::Button::new(
                                RichText::new("Выбран")
                                    .strong()
                                    .color(Color32::from_rgb(179, 225, 205)),
                            )
                            .fill(Color32::from_rgb(35, 64, 57))
                            .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.32)))
                        } else {
                            egui::Button::new(
                                RichText::new("Переключить")
                                    .strong()
                                    .color(Color32::from_rgb(203, 232, 217)),
                            )
                            .fill(Color32::from_rgb(35, 67, 58))
                            .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.36)))
                        }
                        .corner_radius(9);
                        if ui
                            .add_enabled(self.busy.is_none() && !is_active, switch_button)
                            .clicked()
                        {
                            let core = self.core.clone();
                            let id = account.id.clone();
                            self.play("switch");
                            self.start_dashboard_action(
                                "Переключение аккаунта",
                                async move { commands::switch_account(&core, id).await },
                            );
                        }
                        let remove = ui
                            .add_enabled(
                                self.busy.is_none(),
                                egui::Button::new("")
                                    .min_size(Vec2::splat(30.0))
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE)
                                    .corner_radius(8),
                            )
                            .on_hover_text("Удалить аккаунт");
                        remove.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                remove.enabled(),
                                "Удалить аккаунт",
                            )
                        });
                        let remove_color = if remove.hovered() {
                            RED
                        } else {
                            MUTED.gamma_multiply(0.78)
                        };
                        paint_header_icon(
                            ui.painter(),
                            remove.rect.shrink(8.0),
                            HeaderIcon::Trash,
                            remove_color,
                        );
                        if remove.clicked() {
                            self.pending_remove = Some((account.id.clone(), account.email.clone()));
                            self.begin_confirmation(ConfirmAction::RemoveAccount);
                        }
                    });
                });
                ui.add_space(8.0);
                self.render_usage_line(
                    ui,
                    "account-card",
                    &account.id,
                    "Короткое окно",
                    account
                        .usage
                        .as_ref()
                        .and_then(|usage| usage.primary.as_ref()),
                );
                ui.add_space(5.0);
                self.render_usage_line(
                    ui,
                    "account-card",
                    &account.id,
                    "Длинное окно",
                    account
                        .usage
                        .as_ref()
                        .and_then(|usage| usage.secondary.as_ref()),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Обновлено").color(MUTED).size(9.0));
                    ui.label(
                        RichText::new(
                            account
                                .usage
                                .as_ref()
                                .map(|usage| format_updated(usage.updated_at))
                                .unwrap_or_else(|| "ожидается проверка".to_owned()),
                        )
                        .color(TEXT)
                        .size(9.0),
                    );
                    if let Some(error) = account
                        .usage
                        .as_ref()
                        .and_then(|usage| usage.error.as_deref())
                    {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new("ПРОВЕРИТЬ").color(RED).size(9.0))
                                .on_hover_text(error);
                        });
                    }
                });
            });
        let rect = response.response.rect;
        let glow = ui.ctx().animate_value_with_time(
            egui::Id::unique(("account-pulse", &account.id)),
            if is_active { 1.0 } else { 0.0 },
            0.35,
        );
        if glow > 0.01 {
            ui.painter().line_segment(
                [
                    rect.left_top() + Vec2::new(12.0, 0.0),
                    rect.right_top() - Vec2::new(12.0, 0.0),
                ],
                Stroke::new(glow, ACCENT.gamma_multiply(0.48)),
            );
        }
        if let Some(started_at) = self.reorder_animations.get(&account.id).copied() {
            let progress = (started_at.elapsed().as_secs_f32() / 0.48).clamp(0.0, 1.0);
            if progress < 1.0 {
                let alpha = (1.0 - progress).powi(2) * 0.45;
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(12),
                    Stroke::new(1.8, ACCENT.gamma_multiply(alpha)),
                    egui::StrokeKind::Inside,
                );
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            } else {
                self.reorder_animations.remove(&account.id);
            }
        }
        if response.response.hovered() && !is_active && self.busy.is_none() {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(12),
                Stroke::new(1.0, ACCENT.gamma_multiply(0.18)),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn render_usage_line(
        &self,
        ui: &mut egui::Ui,
        animation_scope: &'static str,
        account_id: &str,
        label: &str,
        window: Option<&UsageWindow>,
    ) {
        let left = window.map(remaining_percent);
        let tone = left.map(quota_color).unwrap_or(MUTED);
        ui.horizontal(|ui| {
            ui.set_height(20.0);
            ui.label(RichText::new(label).size(10.0).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(
                        left.map(|value| format!("{value}% осталось"))
                            .unwrap_or_else(|| "нет данных".to_owned()),
                    )
                    .strong()
                    .size(11.0)
                    .color(tone),
                );
                if let Some(window) = window {
                    ui.label(
                        RichText::new(format!("сброс {}", format_reset(window.resets_at)))
                            .size(9.0)
                            .color(MUTED),
                    );
                }
            });
        });
        let available = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(available, 6.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), Color32::from_rgb(40, 59, 57));
        if let Some(value) = left {
            let fill_fraction = ui.ctx().animate_value_with_time(
                egui::Id::unique(("quota-fill", animation_scope, account_id, label)),
                value as f32 / 100.0,
                0.24,
            );
            let fill = egui::Rect::from_min_size(
                rect.min,
                Vec2::new(rect.width() * fill_fraction, rect.height()),
            );
            ui.painter().rect_filled(fill, CornerRadius::same(4), tone);
        }
    }

    fn render_summary(&mut self, ui: &mut egui::Ui, data: &DashboardData) {
        let column_height = ui.available_height();
        let summary_height = (column_height * 0.42).clamp(245.0, 330.0);
        let active = data
            .accounts
            .iter()
            .find(|account| account.id == data.current_account_id.as_deref().unwrap_or_default());
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_min_height(summary_height);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("ТЕКУЩИЙ СЕАНС")
                                .color(MUTED)
                                .size(10.0)
                                .strong()
                                .monospace(),
                        );
                        ui.label(
                            RichText::new(if data.codex_connected {
                                "● МАРШРУТ ПОДКЛЮЧЁН"
                            } else {
                                "○ CODEX НЕ ПОДКЛЮЧЁН"
                            })
                            .color(if data.codex_connected { ACCENT } else { GOLD })
                            .size(9.0),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(if data.codex_running {
                                "CODEX РАБОТАЕТ"
                            } else {
                                "ОЖИДАЕТ CODEX"
                            })
                            .color(MUTED)
                            .size(9.0)
                            .monospace(),
                        );
                    });
                });
                ui.add_space(13.0);
                if let Some(account) = active {
                    ui.horizontal(|ui| {
                        let (avatar, _) =
                            ui.allocate_exact_size(Vec2::splat(44.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            avatar,
                            CornerRadius::same(13),
                            ACCENT.gamma_multiply(0.18),
                        );
                        ui.painter().rect_stroke(
                            avatar,
                            CornerRadius::same(13),
                            Stroke::new(1.0, ACCENT.gamma_multiply(0.42)),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            avatar.center(),
                            egui::Align2::CENTER_CENTER,
                            account
                                .email
                                .chars()
                                .next()
                                .unwrap_or('?')
                                .to_uppercase()
                                .to_string(),
                            FontId::proportional(20.0),
                            ACCENT,
                        );
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&account.email).strong().size(16.0));
                            ui.label(RichText::new(&account.plan).color(MUTED));
                        });
                    });
                    ui.add_space(18.0);
                    let available = account_remaining(account);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("ЛУЧШИЙ ОСТАТОК")
                                .color(MUTED)
                                .size(9.0)
                                .monospace(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(
                                    available
                                        .map(|value| format!("{value}%"))
                                        .unwrap_or_else(|| "—".to_owned()),
                                )
                                .strong()
                                .size(26.0)
                                .color(available.map(quota_color).unwrap_or(MUTED)),
                            );
                        });
                    });
                    self.render_usage_line(
                        ui,
                        "active-summary",
                        &account.id,
                        "Короткое окно",
                        account
                            .usage
                            .as_ref()
                            .and_then(|usage| usage.primary.as_ref()),
                    );
                    ui.add_space(8.0);
                    self.render_usage_line(
                        ui,
                        "active-summary",
                        &account.id,
                        "Длинное окно",
                        account
                            .usage
                            .as_ref()
                            .and_then(|usage| usage.secondary.as_ref()),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Следующий сброс").color(MUTED).size(10.0));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format_reset(next_reset(account)))
                                    .color(TEXT)
                                    .size(11.0),
                            );
                        });
                    });
                } else {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(
                            "Добавьте аккаунт, чтобы увидеть активный профиль и его лимиты.",
                        )
                        .color(MUTED),
                    );
                }
                ui.add_space(10.0);
                let status = if data.codex_connected {
                    "Переключение доступно между запросами."
                } else {
                    "Для переключения подключите Codex один раз."
                };
                ui.label(RichText::new(status).color(MUTED).size(10.0));
                if !data.codex_connected
                    && !data.accounts.is_empty()
                    && self.primary_button(ui, "Подключить Codex")
                {
                    let core = self.core.clone();
                    self.start_dashboard_action("Подключение Codex", async move {
                        commands::connect_codex(&core).await
                    });
                }
                if data.codex_connected && self.secondary_button(ui, "Отключить локальный маршрут")
                {
                    let core = self.core.clone();
                    self.start_dashboard_action("Отключение Codex", async move {
                        commands::disconnect_codex(&core).await
                    });
                }
                if let Some(error) = &data.proxy_error {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("Маршрут: {error}"))
                            .color(RED)
                            .size(10.0),
                    );
                }
            });
        ui.add_space(10.0);
        self.render_activity(ui, data);
        ui.add_space(8.0);
        let refresh_height = ui.available_height();
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(0.7, BORDER))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_min_height(refresh_height.max(86.0));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("ПРОВЕРКА ЛИМИТОВ")
                            .color(MUTED)
                            .size(9.0)
                            .monospace(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "каждые {}",
                                format_interval(data.settings.refresh_interval_seconds)
                            ))
                            .color(ACCENT)
                            .size(10.0),
                        );
                    });
                });
                let refreshed = data
                    .refreshed_at
                    .map(format_updated)
                    .unwrap_or_else(|| "ещё не выполнялось".into());
                ui.label(
                    RichText::new(format!("Последнее обновление: {refreshed}"))
                        .color(MUTED)
                        .size(10.0),
                );
            });
    }

    fn render_activity(&mut self, ui: &mut egui::Ui, data: &DashboardData) {
        let activity_height = (ui.available_height() * 0.56).clamp(165.0, 245.0);
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(15, 12))
            .show(ui, |ui| {
                ui.set_min_height(activity_height);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("ИСТОРИЯ ДЕЙСТВИЙ")
                                .color(MUTED)
                                .size(10.0)
                                .monospace(),
                        );
                        ui.heading("События");
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{:02}", self.toast_events.len()))
                                .color(ACCENT)
                                .monospace(),
                        );
                    });
                });
                ui.add_space(7.0);
                if self.toast_events.is_empty() {
                    ui.label(
                        RichText::new("Обновления лимитов и переключения будут появляться здесь.")
                            .color(MUTED)
                            .size(10.0),
                    );
                }
                for (title, detail, error, at) in self.toast_events.iter().take(4) {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format_clock(*at))
                                .color(MUTED)
                                .size(10.0)
                                .monospace(),
                        );
                        ui.label(
                            RichText::new("●")
                                .color(if *error { RED } else { ACCENT })
                                .size(10.0),
                        );
                        ui.vertical(|ui| {
                            ui.label(RichText::new(title).strong().size(10.0));
                            ui.label(RichText::new(detail).color(MUTED).size(10.0));
                        });
                    });
                }
                if self.last_sync.elapsed() < Duration::from_secs(2) && data.refreshed_at.is_some()
                {
                    let alpha = self.context.animate_value_with_time(
                        egui::Id::unique("sync-indicator"),
                        0.2,
                        0.7,
                    );
                    ui.painter().circle_filled(
                        ui.min_rect().right_top() - Vec2::new(15.0, -12.0),
                        2.5,
                        ACCENT.gamma_multiply(alpha),
                    );
                }
            });
    }

    fn render_overlay(&mut self, context: &egui::Context) {
        if let Some(panel) = self.panel {
            let title = match panel {
                Panel::Settings => "Настройки приложения",
                Panel::Codex => "Настройки Codex",
                Panel::Backup => "Резервная копия",
            };
            let mut open = true;
            egui::Window::new(title)
                .id(egui::Id::unique("settings-modal"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_size(Vec2::new(660.0, 700.0))
                .min_size(Vec2::new(540.0, 500.0))
                .max_width(760.0)
                .show(context, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(650.0)
                        .show(ui, |ui| match panel {
                            Panel::Settings => self.render_settings_panel(ui),
                            Panel::Codex => self.render_codex_panel(ui),
                            Panel::Backup => self.render_backup_panel(ui),
                        });
                });
            if !open {
                self.panel = None;
            }
        }
        if let Some(action) = self.confirmation {
            let (title, message, labels) = match action {
                ConfirmAction::Exit => ("Закрыть Codex Accounts?", "Исходный адрес Codex будет восстановлен. Активный запрос может прерваться.", ["Выйти сейчас", "Подтвердить выход", "Закрыть приложение"]),
                ConfirmAction::ResetSettings => ("Сбросить настройки?", "Автозапуск, интервал, порог и подключение Codex вернутся к значениям по умолчанию. Аккаунты сохранятся.", ["Проверить сброс", "Подтвердить сброс", "Сбросить настройки"]),
                ConfirmAction::ResetApplication => ("Полный сброс приложения", "Удалятся сохранённые аккаунты, настройки и пароль администратора. Codex будет отключён.", ["Проверить данные", "Подтверждаю удаление", "Выполнить полный сброс"]),
                ConfirmAction::RemoveAccount => ("Удалить аккаунт?", "Сохранённый профиль будет удалён из этого приложения.", ["Удалить профиль", "Удалить профиль", "Удалить профиль"]),
            };
            let mut open = true;
            egui::Window::new(title)
                .id(egui::Id::unique("confirm-modal"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(435.0)
                .show(context, |ui| {
                    ui.label(RichText::new(message).color(MUTED));
                    if action == ConfirmAction::RemoveAccount {
                        if let Some((_, email)) = &self.pending_remove {
                            ui.add_space(7.0);
                            ui.label(RichText::new(email).strong());
                        }
                    }
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(format!(
                            "Подтверждение {}/{}",
                            self.confirmation_step + 1,
                            match action {
                                ConfirmAction::Exit => 1,
                                ConfirmAction::ResetSettings => 2,
                                ConfirmAction::ResetApplication => 3,
                                ConfirmAction::RemoveAccount => 1,
                            }
                        ))
                        .color(GOLD)
                        .size(10.0),
                    );
                    ui.horizontal(|ui| {
                        if self.danger_button(ui, labels[self.confirmation_step.min(2) as usize]) {
                            self.advance_confirmation();
                        }
                        if self.secondary_button(ui, "Отмена") {
                            self.confirmation = None;
                            self.confirmation_step = 0;
                            self.pending_remove = None;
                            self.play("dialog_cancel");
                        }
                    });
                });
            if !open {
                self.confirmation = None;
                self.pending_remove = None;
            }
        }
        if let Some(toast) = &self.toast {
            let age = toast.at.elapsed().as_secs_f32();
            let alpha = (age / 0.16).min(1.0) * ((6.0 - age) / 0.25).clamp(0.0, 1.0);
            egui::Area::new(egui::Id::unique("native-toast"))
                .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -18.0))
                .order(egui::Order::Foreground)
                .show(context, |ui| {
                    egui::Frame::new()
                        .fill(if toast.error {
                            Color32::from_rgb(58, 35, 34).gamma_multiply(alpha)
                        } else {
                            Color32::from_rgb(27, 49, 43).gamma_multiply(alpha)
                        })
                        .stroke(Stroke::new(
                            alpha,
                            if toast.error {
                                RED.gamma_multiply(0.45 * alpha)
                            } else {
                                ACCENT.gamma_multiply(0.35 * alpha)
                            },
                        ))
                        .corner_radius(12)
                        .inner_margin(egui::Margin::symmetric(15, 11))
                        .show(ui, |ui| {
                            ui.set_max_width(390.0);
                            ui.label(RichText::new(&toast.title).strong().color(if toast.error {
                                RED.gamma_multiply(alpha)
                            } else {
                                ACCENT.gamma_multiply(alpha)
                            }));
                            ui.label(
                                RichText::new(&toast.detail)
                                    .size(11.0)
                                    .color(TEXT.gamma_multiply(alpha)),
                            );
                        });
                });
        }
    }

    fn render_settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("ПОВЕДЕНИЕ ПРИЛОЖЕНИЯ")
                .color(MUTED)
                .monospace()
                .size(9.0),
        );
        ui.add_space(5.0);
        ui.heading("Запуск и лимиты");
        ui.label(
            RichText::new("Частота обновления, порог переключения и доступ к приложению.")
                .color(MUTED)
                .size(11.0),
        );
        ui.add_space(12.0);
        self.setting_toggle(
            ui,
            "Автозапуск с Windows",
            "Запускать после входа в систему",
            SettingField::Startup,
        );
        self.setting_toggle(
            ui,
            "Сразу в системный трей",
            "Запускать в фоне без открытого окна",
            SettingField::StartInTray,
        );
        self.setting_toggle(
            ui,
            "Автопереключение аккаунтов",
            "Перейти к доступному аккаунту при низком лимите",
            SettingField::AutomaticSwitching,
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "Переключать при остатке: {}%",
                self.settings.switch_threshold
            ))
            .color(TEXT),
        );
        let mut threshold = self.settings.switch_threshold as f32;
        if ui
            .add(egui::Slider::new(&mut threshold, 0.0..=100.0).suffix("%"))
            .changed()
        {
            self.settings.switch_threshold = threshold.round() as u8;
        }
        self.render_interval_picker(ui);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if self.primary_button(ui, "Сохранить настройки") {
                self.update_settings();
            }
            if self.secondary_button(ui, "Значения по умолчанию") {
                self.begin_confirmation(ConfirmAction::ResetSettings);
            }
            if self.danger_button(ui, "Полный сброс") {
                self.begin_confirmation(ConfirmAction::ResetApplication);
            }
        });
        ui.separator();
        ui.heading("Защита паролем");
        let protected = self
            .dashboard
            .as_ref()
            .is_some_and(|data| data.admin_protected);
        ui.label(
            RichText::new(if protected {
                "Пароль включён. При скрытии окна данные закрываются."
            } else {
                "Необязательный пароль для аккаунтов и настроек."
            })
            .color(MUTED)
            .size(10.0),
        );
        if protected {
            password_field(ui, "Текущий пароль", &mut self.current_password);
        }
        password_field(
            ui,
            if protected {
                "Новый пароль"
            } else {
                "Задайте пароль"
            },
            &mut self.new_password,
        );
        password_field(ui, "Повторите пароль", &mut self.confirm_password);
        ui.horizontal(|ui| {
            if self.primary_button(
                ui,
                if protected {
                    "Изменить пароль"
                } else {
                    "Включить защиту"
                },
            ) {
                self.configure_password(false);
            }
            if protected && self.danger_button(ui, "Отключить защиту") {
                self.configure_password(true);
            }
        });
    }

    fn setting_toggle(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        detail: &str,
        field: SettingField,
    ) {
        let value = match field {
            SettingField::Startup => self.settings.start_with_windows,
            SettingField::StartInTray => self.settings.start_in_tray,
            SettingField::AutomaticSwitching => self.settings.automatic_switching,
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).color(TEXT));
                ui.label(RichText::new(detail).color(MUTED).size(10.0));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut enabled = value;
                let response = ui.checkbox(&mut enabled, "");
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::Role::CheckBox,
                        ui.is_enabled(),
                        enabled,
                        title,
                    )
                });
                if response.changed() {
                    match field {
                        SettingField::Startup => {
                            self.settings.start_with_windows = enabled;
                            if !enabled {
                                self.settings.start_in_tray = false;
                            }
                        }
                        SettingField::StartInTray => self.settings.start_in_tray = enabled,
                        SettingField::AutomaticSwitching => {
                            self.settings.automatic_switching = enabled
                        }
                    }
                    self.play(match field {
                        SettingField::Startup => "startup_toggle",
                        SettingField::StartInTray => "tray_toggle",
                        SettingField::AutomaticSwitching => "failover_toggle",
                    });
                }
            });
        });
        ui.separator();
    }

    fn configure_password(&mut self, disable: bool) {
        if !disable && (self.new_password.len() < 8 || self.new_password != self.confirm_password) {
            self.notify(
                "Пароль не принят",
                "Введите одинаковые пароли длиной не менее 8 символов.",
                true,
            );
            return;
        }
        let current =
            (!self.current_password.is_empty()).then(|| std::mem::take(&mut self.current_password));
        let new = if disable {
            None
        } else {
            Some(std::mem::take(&mut self.new_password))
        };
        self.confirm_password.clear();
        let core = self.core.clone();
        self.start_dashboard_action("Защита паролем", async move {
            commands::configure_admin_password(&core, current, new).await
        });
    }

    fn render_codex_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("ОБЩИЕ ПАРАМЕТРЫ CODEX")
                .color(MUTED)
                .monospace()
                .size(9.0),
        );
        ui.heading("Настройки Codex");
        ui.label(
            RichText::new("Эти параметры и prompts общие при переключении аккаунта.")
                .color(MUTED)
                .size(10.0),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label("Модель");
            ui.add(
                egui::TextEdit::singleline(&mut self.preferences.model)
                    .desired_width(300.0)
                    .hint_text("По умолчанию Codex"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Уровень рассуждений");
            egui::ComboBox::from_id_salt("reasoning-level")
                .selected_text(if self.preferences.model_reasoning_effort.is_empty() {
                    "По умолчанию"
                } else {
                    &self.preferences.model_reasoning_effort
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        ("", "По умолчанию"),
                        ("minimal", "Минимальный"),
                        ("low", "Низкий"),
                        ("medium", "Средний"),
                        ("high", "Высокий"),
                        ("xhigh", "Очень высокий"),
                        ("max", "Максимальный"),
                        ("ultra", "Ultra"),
                    ] {
                        ui.selectable_value(
                            &mut self.preferences.model_reasoning_effort,
                            value.to_owned(),
                            label,
                        );
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Песочница");
            egui::ComboBox::from_id_salt("sandbox-mode")
                .selected_text(if self.preferences.sandbox_mode.is_empty() {
                    "По умолчанию"
                } else {
                    &self.preferences.sandbox_mode
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        ("", "По умолчанию"),
                        ("read-only", "Только чтение"),
                        ("workspace-write", "Запись в рабочую папку"),
                        ("danger-full-access", "Полный доступ"),
                    ] {
                        ui.selectable_value(
                            &mut self.preferences.sandbox_mode,
                            value.to_owned(),
                            label,
                        );
                    }
                });
            ui.label("Подтверждение");
            egui::ComboBox::from_id_salt("approval-policy")
                .selected_text(if self.preferences.approval_policy.is_empty() {
                    "По умолчанию"
                } else {
                    &self.preferences.approval_policy
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        ("", "По умолчанию"),
                        ("on-request", "По запросу"),
                        ("never", "Без подтверждения"),
                    ] {
                        ui.selectable_value(
                            &mut self.preferences.approval_policy,
                            value.to_owned(),
                            label,
                        );
                    }
                });
        });
        ui.label("Общая инструкция");
        ui.add(
            egui::TextEdit::multiline(&mut self.preferences.developer_instructions)
                .desired_rows(4)
                .hint_text("Инструкция, применяемая ко всем аккаунтам…"),
        );
        if self.primary_button(ui, "Сохранить настройки Codex") {
            let core = self.core.clone();
            let preferences = self.preferences.clone();
            self.start_dashboard_action("Настройки Codex", async move {
                commands::save_codex_preferences(&core, preferences).await
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.heading("Библиотека prompts");
            if self.secondary_button(ui, "+ Новый prompt") {
                self.new_prompt = true;
                self.prompt_name.clear();
                self.prompt_content.clear();
                self.play("prompt_add");
            }
        });
        if !self.prompts.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (index, prompt) in self.prompts.iter().enumerate() {
                    if ui
                        .selectable_label(
                            !self.new_prompt && self.selected_prompt == index,
                            format!("/{}", prompt.name),
                        )
                        .clicked()
                    {
                        self.selected_prompt = index;
                        self.new_prompt = false;
                        self.prompt_name = prompt.name.clone();
                        self.prompt_content = prompt.content.clone();
                        self.play("prompt_select");
                    }
                }
            });
        }
        if self.new_prompt || !self.prompts.is_empty() {
            ui.horizontal(|ui| {
                ui.label("Команда");
                ui.add(
                    egui::TextEdit::singleline(&mut self.prompt_name)
                        .desired_width(210.0)
                        .hint_text("review"),
                );
            });
            ui.label("Текст команды");
            ui.add(
                egui::TextEdit::multiline(&mut self.prompt_content)
                    .desired_rows(5)
                    .hint_text("Повторяемая инструкция для Codex…"),
            );
            ui.horizontal(|ui| {
                if self.primary_button(
                    ui,
                    if self.new_prompt {
                        "Создать prompt"
                    } else {
                        "Сохранить prompt"
                    },
                ) {
                    if !self
                        .prompt_name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                        || self.prompt_name.is_empty()
                    {
                        self.notify(
                            "Имя prompt не подходит",
                            "Используйте латинские буквы, цифры, дефис или подчёркивание.",
                            true,
                        );
                    } else {
                        let core = self.core.clone();
                        let name = self.prompt_name.clone();
                        let content = self.prompt_content.clone();
                        self.start_dashboard_action("Сохранение prompt", async move {
                            commands::save_prompt(&core, name, content).await
                        });
                        self.new_prompt = false;
                    }
                }
                if !self.new_prompt && self.danger_button(ui, "Удалить prompt") {
                    let core = self.core.clone();
                    let name = self.prompt_name.clone();
                    self.play("prompt_remove_confirm");
                    self.start_dashboard_action("Удаление prompt", async move {
                        commands::delete_prompt(&core, name).await
                    });
                }
            });
        } else {
            ui.label(
                RichText::new("Добавьте Markdown‑команду, например /review или /deploy.")
                    .color(MUTED),
            );
        }
    }

    fn render_backup_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("ЛОКАЛЬНЫЙ ЭКСПОРТ")
                .color(MUTED)
                .monospace()
                .size(9.0),
        );
        ui.heading("Резервная копия");
        ui.label(
            RichText::new("Копия включает аккаунты, лимиты, параметры Codex и prompts.")
                .color(MUTED)
                .size(10.0),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label("Пароль шифрования");
            ui.add(
                egui::TextEdit::singleline(&mut self.backup_password)
                    .password(true)
                    .desired_width(240.0)
                    .hint_text("Пусто — без шифрования"),
            );
        });
        ui.label(
            RichText::new(
                "При шифровании задайте не менее 8 символов. Пароль потребуется для импорта.",
            )
            .color(MUTED)
            .size(10.0),
        );
        ui.label(
            RichText::new(
                "Экспорт без пароля содержит данные входа в открытом виде. Токены внутри приложения защищены Windows DPAPI.",
            )
            .color(GOLD)
            .size(10.0),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if self.primary_button(ui, "Экспортировать аккаунты") {
                self.export_backup();
            }
            if self.secondary_button(ui, "Импортировать резервную копию")
            {
                self.import_backup();
            }
        });
        ui.add_space(12.0);
        ui.label(
            RichText::new("Данные аккаунтов хранятся локально с защитой Windows DPAPI.")
                .color(MUTED)
                .size(10.0),
        );
    }

    fn export_backup(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Сохранить резервную копию")
            .add_filter("Резервная копия Codex Accounts", &["codexaccounts", "json"])
            .set_file_name("codex-accounts-backup.codexaccounts")
            .save_file()
        else {
            return;
        };
        let core = self.core.clone();
        let password =
            (!self.backup_password.is_empty()).then(|| std::mem::take(&mut self.backup_password));
        self.start_action("Экспорт резервной копии", async move {
            commands::export_backup(&core, path.display().to_string(), password).await?;
            commands::load_dashboard(&core).await.map(Some)
        });
    }

    fn import_backup(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Открыть резервную копию")
            .add_filter("Резервная копия Codex Accounts", &["codexaccounts", "json"])
            .pick_file()
        else {
            return;
        };
        let core = self.core.clone();
        let password =
            (!self.backup_password.is_empty()).then(|| std::mem::take(&mut self.backup_password));
        self.start_dashboard_action("Импорт резервной копии", async move {
            commands::import_backup(&core, path.display().to_string(), password).await
        });
    }

    fn render_tray_and_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
            paint_header_icon(ui.painter(), icon_rect.shrink(2.0), HeaderIcon::Lock, MUTED);
            ui.label(
                RichText::new("УЧЁТНЫЕ ДАННЫЕ ЗАЩИЩЕНЫ WINDOWS DPAPI")
                    .color(MUTED)
                    .size(9.0)
                    .monospace(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.tray.is_some(),
                        egui::Button::new(
                            RichText::new("—  В трей")
                                .color(if self.tray.is_some() { MUTED } else { BORDER })
                                .size(10.0),
                        ),
                    )
                    .clicked()
                {
                    self.core.lock_admin();
                    self.window_transition = Some(WindowTransition::Hide(Instant::now()));
                    self.play("minimize");
                }
                if self.busy.is_some() {
                    ui.spinner();
                }
                let _keep_tray_alive = self.tray.is_some();
            });
        });
    }

    fn primary_button(&self, ui: &mut egui::Ui, label: &str) -> bool {
        let button = egui::Button::new(
            RichText::new(label)
                .strong()
                .color(Color32::from_rgb(13, 31, 28)),
        )
        .fill(ACCENT)
        .stroke(Stroke::new(1.0, ACCENT))
        .corner_radius(9);
        ui.add_enabled(self.busy.is_none(), button).clicked()
    }

    fn secondary_button(&self, ui: &mut egui::Ui, label: &str) -> bool {
        ui.add_enabled(
            self.busy.is_none(),
            egui::Button::new(RichText::new(label).color(TEXT))
                .fill(SURFACE_HI)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(9),
        )
        .clicked()
    }

    fn danger_button(&self, ui: &mut egui::Ui, label: &str) -> bool {
        ui.add_enabled(
            self.busy.is_none(),
            egui::Button::new(RichText::new(label).color(RED))
                .fill(Color32::from_rgb(48, 34, 34))
                .stroke(Stroke::new(1.0, RED.gamma_multiply(0.35)))
                .corner_radius(9),
        )
        .clicked()
    }

    fn paint_window_transition(&mut self, context: &egui::Context) {
        let Some(transition) = self.window_transition else {
            return;
        };
        let started_at = match transition {
            WindowTransition::Hide(started_at)
            | WindowTransition::Show(started_at)
            | WindowTransition::Close(started_at) => started_at,
        };
        let progress = (started_at.elapsed().as_secs_f32()
            / WINDOW_TRANSITION_DURATION.as_secs_f32())
        .clamp(0.0, 1.0);
        let opacity = match transition {
            WindowTransition::Show(_) => 1.0 - progress,
            WindowTransition::Hide(_) | WindowTransition::Close(_) => progress,
        };
        if opacity > 0.01 {
            context
                .layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    self.transition_layer_id,
                ))
                .rect_filled(
                    context.content_rect(),
                    CornerRadius::ZERO,
                    Color32::from_black_alpha((opacity * 220.0) as u8),
                );
        }
        if progress >= 1.0 {
            match transition {
                WindowTransition::Hide(_) => {
                    context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                WindowTransition::Show(_) => {}
                WindowTransition::Close(_) => {
                    context.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            self.window_transition = None;
            context.request_repaint();
        } else {
            context.request_repaint_after(Duration::from_millis(16));
        }
    }
}

#[derive(Clone, Copy)]
enum SettingField {
    Startup,
    StartInTray,
    AutomaticSwitching,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IntervalUnit {
    Hours,
    Minutes,
    Seconds,
}

impl IntervalUnit {
    fn label(self) -> &'static str {
        match self {
            Self::Hours => "часы",
            Self::Minutes => "минуты",
            Self::Seconds => "секунды",
        }
    }

    fn input_id(self) -> &'static str {
        match self {
            Self::Hours => "hours",
            Self::Minutes => "minutes",
            Self::Seconds => "seconds",
        }
    }

    fn component_index(self) -> usize {
        match self {
            Self::Hours => 0,
            Self::Minutes => 1,
            Self::Seconds => 2,
        }
    }

    fn next(self) -> Option<Self> {
        match self {
            Self::Hours => Some(Self::Minutes),
            Self::Minutes => Some(Self::Seconds),
            Self::Seconds => None,
        }
    }
}

#[derive(Default)]
struct IntervalPickerState {
    component_text: [String; 3],
    drag: Option<IntervalWheelDrag>,
    scroll_remainder: Option<(IntervalUnit, f32)>,
}

#[derive(Clone, Copy)]
struct IntervalWheelDrag {
    unit: IntervalUnit,
    remainder: f32,
}

impl eframe::App for NativeApp {
    fn logic(&mut self, _context: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe calls logic while a viewport is hidden; tray events must be
        // consumed here so restore and quit still work without a visible frame.
        self.poll_messages();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let close_requested = ui.input(|input| input.viewport().close_requested());
        if close_requested && !self.exiting && self.confirmation != Some(ConfirmAction::Exit) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.begin_confirmation(ConfirmAction::Exit);
        }
        if close_requested && self.confirmation == Some(ConfirmAction::Exit) && !self.exiting {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let hidden = ui.input(|input| input.viewport().minimized == Some(true));
        let restored_from_minimized = self.was_minimized && !hidden;
        if self.was_minimized && !hidden {
            self.window_transition = Some(WindowTransition::Show(Instant::now()));
            self.play("restore");
        } else if !self.was_minimized && hidden {
            self.play("minimize");
        }
        self.was_minimized = hidden;
        let maximized = ui.input(|input| input.viewport().maximized == Some(true));
        if !hidden && !restored_from_minimized && maximized != self.was_maximized {
            self.window_transition = Some(WindowTransition::Show(Instant::now()));
            self.play(if maximized { "maximize" } else { "restore" });
        }
        self.was_maximized = maximized;
        if hidden {
            self.core.lock_admin();
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::same(15)),
            )
            .show(ui, |ui| {
                self.render_header(ui);
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .show(ui, |ui| self.render_dashboard(ui));
                ui.add_space(7.0);
                self.render_tray_and_status(ui);
            });
        self.render_overlay(ui.ctx());
        self.paint_window_transition(ui.ctx());
        if self.busy.is_some() || self.toast.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(220));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.core.lock_admin();
        let _keep_tray_alive = self.tray.take();
    }
}

fn password_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT).size(11.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(
                egui::TextEdit::singleline(value)
                    .password(true)
                    .desired_width(240.0),
            );
        });
    });
}

pub(crate) fn create_tray() -> Result<TrayIcon, String> {
    let show = MenuItem::with_id("show", "Открыть Codex Accounts", true, None);
    let refresh = MenuItem::with_id("refresh", "Обновить лимиты", true, None);
    let quit = MenuItem::with_id("quit", "Выйти…", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &refresh, &quit])
        .map_err(|_| "Не удалось создать меню значка в области уведомлений.".to_string())?;
    let icon = Icon::from_rgba(tray_icon_rgba(), 32, 32)
        .map_err(|_| "Не удалось подготовить изображение значка системного трея.".to_string())?;
    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Codex Accounts · локальный менеджер")
        .with_icon(icon)
        .build()
        .map_err(|_| {
            "Windows не создал значок системного трея. Проверьте, что область уведомлений доступна."
                .to_string()
        })
}

fn paint_header_icon(painter: &egui::Painter, rect: egui::Rect, icon: HeaderIcon, color: Color32) {
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.42;
    let stroke = Stroke::new(1.8, color);
    match icon {
        HeaderIcon::Refresh => {
            let start = -std::f32::consts::FRAC_PI_3;
            let end = std::f32::consts::TAU - std::f32::consts::FRAC_PI_3;
            let points = (0..=24)
                .map(|step| {
                    let angle = start + (end - start) * step as f32 / 24.0;
                    center + Vec2::new(angle.cos(), angle.sin()) * radius
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
            let tip = center + Vec2::new(start.cos(), start.sin()) * radius;
            let tangent = Vec2::new(-start.sin(), start.cos());
            let base = tip - tangent * 4.0;
            painter.add(egui::Shape::convex_polygon(
                vec![tip, base + Vec2::new(0.0, -4.0), base + Vec2::new(0.0, 4.0)],
                color,
                Stroke::NONE,
            ));
        }
        HeaderIcon::Backup => {
            let tray_top = center.y + radius * 0.15;
            painter.line_segment(
                [
                    egui::pos2(center.x - radius, tray_top),
                    egui::pos2(center.x - radius, center.y + radius * 0.8),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x + radius, tray_top),
                    egui::pos2(center.x + radius, center.y + radius * 0.8),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - radius, center.y + radius * 0.8),
                    egui::pos2(center.x + radius, center.y + radius * 0.8),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x, center.y + radius * 0.25),
                    egui::pos2(center.x, center.y - radius),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x, center.y - radius),
                    egui::pos2(center.x - radius * 0.38, center.y - radius * 0.62),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x, center.y - radius),
                    egui::pos2(center.x + radius * 0.38, center.y - radius * 0.62),
                ],
                stroke,
            );
        }
        HeaderIcon::Codex => {
            for (index, offset) in [-0.62_f32, 0.0, 0.62].into_iter().enumerate() {
                let y = center.y + radius * offset;
                let left = center.x - radius;
                let right = center.x + radius;
                painter.line_segment([egui::pos2(left, y), egui::pos2(right, y)], stroke);
                let knob_x = center.x + radius * [-0.35_f32, 0.42, -0.05][index];
                painter.circle_filled(egui::pos2(knob_x, y), 2.5, BACKGROUND);
                painter.circle_stroke(egui::pos2(knob_x, y), 2.5, stroke);
            }
        }
        HeaderIcon::Settings => {
            for index in 0..8 {
                let angle = std::f32::consts::TAU * index as f32 / 8.0;
                let direction = Vec2::new(angle.cos(), angle.sin());
                painter.line_segment(
                    [
                        center + direction * radius * 0.62,
                        center + direction * radius,
                    ],
                    stroke,
                );
            }
            painter.circle_stroke(center, radius * 0.60, stroke);
            painter.circle_filled(center, radius * 0.19, color);
        }
        HeaderIcon::SoundOn | HeaderIcon::SoundOff => {
            let left = center.x - radius * 0.95;
            let right = center.x - radius * 0.25;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(left, center.y - radius * 0.30),
                    egui::pos2(right, center.y - radius * 0.30),
                    egui::pos2(center.x + radius * 0.42, center.y - radius * 0.85),
                    egui::pos2(center.x + radius * 0.42, center.y + radius * 0.85),
                    egui::pos2(right, center.y + radius * 0.30),
                    egui::pos2(left, center.y + radius * 0.30),
                ],
                color,
                Stroke::NONE,
            ));
            if matches!(icon, HeaderIcon::SoundOn) {
                for wave_radius in [0.65_f32, 1.0] {
                    let points = (0..=12)
                        .map(|step| {
                            let angle = -0.72 + 1.44 * step as f32 / 12.0;
                            center
                                + Vec2::new(angle.cos(), angle.sin()) * radius * wave_radius
                                + Vec2::new(radius * 0.25, 0.0)
                        })
                        .collect();
                    painter.add(egui::Shape::line(points, stroke));
                }
            } else {
                let x = center.x + radius * 0.70;
                let y = radius * 0.30;
                painter.line_segment(
                    [
                        egui::pos2(x - y, center.y - y),
                        egui::pos2(x + y, center.y + y),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        egui::pos2(x + y, center.y - y),
                        egui::pos2(x - y, center.y + y),
                    ],
                    stroke,
                );
            }
        }
        HeaderIcon::Trash => {
            let left = center.x - radius * 0.58;
            let right = center.x + radius * 0.58;
            let top = center.y - radius * 0.42;
            let bottom = center.y + radius * 0.72;
            painter.line_segment(
                [
                    egui::pos2(left, top + radius * 0.22),
                    egui::pos2(left + radius * 0.12, bottom),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(right, top + radius * 0.22),
                    egui::pos2(right - radius * 0.12, bottom),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(left + radius * 0.12, bottom),
                    egui::pos2(right - radius * 0.12, bottom),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(left - radius * 0.1, top),
                    egui::pos2(right + radius * 0.1, top),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - radius * 0.28, top - radius * 0.18),
                    egui::pos2(center.x + radius * 0.28, top - radius * 0.18),
                ],
                stroke,
            );
            for offset in [-0.22_f32, 0.22] {
                let x = center.x + radius * offset;
                painter.line_segment(
                    [
                        egui::pos2(x, top + radius * 0.42),
                        egui::pos2(x, bottom - radius * 0.18),
                    ],
                    Stroke::new(stroke.width * 0.78, color.gamma_multiply(0.72)),
                );
            }
        }
        HeaderIcon::Lock => {
            let body = egui::Rect::from_center_size(
                center + Vec2::new(0.0, radius * 0.28),
                Vec2::new(radius * 1.25, radius * 0.94),
            );
            painter.rect_stroke(
                body,
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(center.x - radius * 0.42, center.y - radius * 0.12),
                    egui::pos2(center.x - radius * 0.42, center.y - radius * 0.54),
                    center + Vec2::new(0.0, -radius * 0.78),
                    egui::pos2(center.x + radius * 0.42, center.y - radius * 0.54),
                    egui::pos2(center.x + radius * 0.42, center.y - radius * 0.12),
                ],
                stroke,
            ));
            painter.circle_filled(center + Vec2::new(0.0, radius * 0.26), radius * 0.08, color);
        }
    }
}

fn tray_icon_rgba() -> Vec<u8> {
    let mut rgba = vec![0_u8; 32 * 32 * 4];
    for y in 0..32_i32 {
        for x in 0..32_i32 {
            let dx = x - 16;
            let dy = y - 16;
            let radius = ((dx * dx + dy * dy) as f32).sqrt();
            let offset = ((y * 32 + x) * 4) as usize;
            if radius < 15.0 {
                rgba[offset..offset + 4].copy_from_slice(&[23, 43, 42, 255]);
            }
            if (radius - 9.0).abs() < 1.7 && (x < 22 || !(10..=21).contains(&y)) {
                rgba[offset..offset + 4].copy_from_slice(&[155, 219, 202, 255]);
            }
            if (20..=25).contains(&x) && (14..=18).contains(&y) {
                rgba[offset..offset + 4].copy_from_slice(&[237, 242, 237, 255]);
            }
        }
    }
    rgba
}

fn remaining_percent(window: &UsageWindow) -> i32 {
    (100 - window.used_percent).clamp(0, 100)
}

fn overview_stat(ui: &mut egui::Ui, label: &str, value: String, color: Color32) {
    ui.vertical(|ui| {
        ui.label(
            RichText::new(label)
                .size(9.0)
                .strong()
                .color(MUTED)
                .monospace(),
        );
        ui.label(RichText::new(value).size(14.0).strong().color(color));
    });
}

fn completion_sound(action: &str) -> &'static str {
    match action {
        "Добавление аккаунта" => "add_complete",
        "Переключение аккаунта" => "switch_complete",
        "Обновление лимитов" => "refresh_complete",
        "Сохранение настроек" => "settings_complete",
        "Подключение Codex" => "connect_complete",
        "Отключение локального маршрута" => "disconnect_complete",
        "Настройки Codex" => "codex_complete",
        "Сохранение prompt" => "prompt_save_complete",
        "Удаление prompt" => "prompt_delete_complete",
        "Сброс настроек" => "settings_reset_complete",
        "Полный сброс" => "app_reset_complete",
        "Защита паролем" => "password_complete",
        "Проверка пароля" => "unlock_complete",
        "Экспорт резервной копии" => "backup_export_complete",
        "Импорт резервной копии" => "backup_import_complete",
        "Выход" => "exit_complete",
        action if action.starts_with("Удаление ") => "account_delete_complete",
        _ => "success",
    }
}

fn action_changes_active_account(action: Option<&str>) -> bool {
    action.is_some_and(|action| {
        matches!(
            action,
            "Добавление аккаунта"
                | "Переключение аккаунта"
                | "Импорт резервной копии"
                | "Полный сброс"
        ) || (action.starts_with("Удаление ") && action != "Удаление prompt")
    })
}

fn error_sound(title: &str) -> &'static str {
    if let Some(action) = title.strip_prefix("Не удалось выполнить: ") {
        return match action {
            "Добавление аккаунта" => "account_add_error",
            "Переключение аккаунта" => "account_switch_error",
            "Обновление лимитов" => "refresh_error",
            "Сохранение настроек" => "settings_error",
            "Звуковые эффекты" => "sound_setting_error",
            "Подключение Codex" => "connect_error",
            "Отключение локального маршрута" => "disconnect_error",
            "Настройки Codex" => "codex_settings_error",
            "Сохранение prompt" => "prompt_save_error",
            "Удаление prompt" => "prompt_delete_error",
            "Сброс настроек" => "settings_reset_error",
            "Полный сброс" => "app_reset_error",
            "Защита паролем" => "password_error",
            "Проверка пароля" => "unlock_error",
            "Экспорт резервной копии" => "backup_export_error",
            "Импорт резервной копии" => "backup_import_error",
            "Выход" => "exit_error",
            action if action.starts_with("Удаление ") => "account_delete_error",
            _ => "error",
        };
    }

    match title {
        "Не удалось обновить данные" => "dashboard_error",
        "Не удалось завершить программу" => "shutdown_error",
        "Локальный маршрут Codex недоступен" => "proxy_error",
        "Доступ закрыт" => "access_error",
        "Неверная частота проверки" => "interval_error",
        "Пароль не принят" | "Введите пароль" => {
            "password_validation_error"
        }
        "Имя prompt не подходит" => "prompt_name_error",
        _ => "error",
    }
}

fn feedback_sound(title: &str, error: bool) -> &'static str {
    if error {
        error_sound(title)
    } else if title == "Подождите завершения действия" {
        "tray_busy"
    } else {
        "success"
    }
}

fn account_remaining(account: &AccountView) -> Option<i32> {
    let values = account
        .usage
        .as_ref()
        .into_iter()
        .flat_map(|usage| [usage.primary.as_ref(), usage.secondary.as_ref()])
        .flatten()
        .map(remaining_percent)
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| *values.iter().max().unwrap_or(&0))
}

fn account_switch_remaining(account: &AccountView) -> Option<i32> {
    account
        .usage
        .as_ref()
        .and_then(crate::model::UsageSnapshot::lowest_remaining)
}

fn quota_color(value: i32) -> Color32 {
    if value <= 5 {
        RED
    } else if value <= 25 {
        GOLD
    } else {
        ACCENT
    }
}

fn next_reset(account: &AccountView) -> Option<i64> {
    account
        .usage
        .as_ref()
        .into_iter()
        .flat_map(|usage| [usage.primary.as_ref(), usage.secondary.as_ref()])
        .flatten()
        .filter(|window| remaining_percent(window) < 100)
        .filter_map(|window| window.resets_at)
        .min()
}

fn format_reset(timestamp: Option<i64>) -> String {
    let Some(timestamp) = timestamp else {
        return "неизвестно".into();
    };
    let reset = chrono::DateTime::from_timestamp(timestamp, 0);
    reset
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%d.%m · %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "неизвестно".into())
}

fn format_updated(timestamp: i64) -> String {
    let minutes = (chrono::Utc::now().timestamp() - timestamp).max(0) / 60;
    match minutes {
        0 => "только что".into(),
        1 => "минуту назад".into(),
        2..=59 => format!("{minutes} мин назад"),
        _ if minutes < 120 => "час назад".into(),
        _ => format!("{} ч назад", minutes / 60),
    }
}

fn draw_interval_picker(
    ui: &mut egui::Ui,
    current: u32,
    state: &mut IntervalPickerState,
) -> Option<(u32, IntervalUnit, i32)> {
    let mut next = current.clamp(1, 86_400);
    let mut changed_by = None;
    egui::Frame::new()
        .fill(Color32::from_rgb(15, 26, 28))
        .stroke(Stroke::new(1.0, BORDER.gamma_multiply(0.75)))
        .corner_radius(14)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Частота проверки лимитов")
                        .color(TEXT)
                        .strong()
                        .size(11.0),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("каждые {}", format_interval(next)))
                            .color(ACCENT)
                            .monospace()
                            .size(10.0),
                    );
                });
            });
            ui.label(
                RichText::new(
                    "Прокрутите или потяните колесо; выбранное значение можно ввести с клавиатуры",
                )
                .color(MUTED)
                .size(9.0),
            );
            ui.add_space(7.0);

            const WHEEL_COLUMN_WIDTH: f32 = 76.0;
            const WHEEL_COLUMN_GAP: f32 = 8.0;
            let wheel_group_width = WHEEL_COLUMN_WIDTH * 3.0 + WHEEL_COLUMN_GAP * 2.0;
            let leading_space = ((ui.available_width() - wheel_group_width) * 0.5).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(leading_space);
                let hours = next / 3_600;
                if let Some(steps) =
                    draw_interval_wheel_column(ui, IntervalUnit::Hours, hours, 24, true, state)
                {
                    let adjusted = adjust_interval_component(next, IntervalUnit::Hours, steps);
                    if adjusted != next {
                        next = adjusted;
                        changed_by = Some((IntervalUnit::Hours, steps.signum()));
                    }
                }

                ui.add_space(WHEEL_COLUMN_GAP);
                let minutes = (next % 3_600) / 60;
                if let Some(steps) = draw_interval_wheel_column(
                    ui,
                    IntervalUnit::Minutes,
                    minutes,
                    59,
                    next < 86_400,
                    state,
                ) {
                    let adjusted = adjust_interval_component(next, IntervalUnit::Minutes, steps);
                    if adjusted != next {
                        next = adjusted;
                        changed_by = Some((IntervalUnit::Minutes, steps.signum()));
                    }
                }

                ui.add_space(WHEEL_COLUMN_GAP);
                let seconds = next % 60;
                if let Some(steps) = draw_interval_wheel_column(
                    ui,
                    IntervalUnit::Seconds,
                    seconds,
                    59,
                    next < 86_400,
                    state,
                ) {
                    let adjusted = adjust_interval_component(next, IntervalUnit::Seconds, steps);
                    if adjusted != next {
                        next = adjusted;
                        changed_by = Some((IntervalUnit::Seconds, steps.signum()));
                    }
                }
            });
            ui.add_space(5.0);
            ui.label(
                RichText::new(
                    "Диапазон: 00:00:01 — 24:00:00 · стрелки ↑/↓ меняют выбранный столбец",
                )
                .color(MUTED)
                .size(8.5),
            );
        });

    changed_by.map(|(unit, direction)| (next, unit, direction))
}

fn draw_interval_wheel_column(
    ui: &mut egui::Ui,
    unit: IntervalUnit,
    value: u32,
    max_value: u32,
    enabled: bool,
    state: &mut IntervalPickerState,
) -> Option<i32> {
    const COLUMN_SIZE: Vec2 = Vec2::new(76.0, 164.0);
    ui.allocate_ui_with_layout(COLUMN_SIZE, Layout::top_down(Align::Center), |column_ui| {
        draw_interval_wheel_column_contents(column_ui, unit, value, max_value, enabled, state)
    })
    .inner
}

fn draw_interval_wheel_column_contents(
    ui: &mut egui::Ui,
    unit: IntervalUnit,
    value: u32,
    max_value: u32,
    enabled: bool,
    state: &mut IntervalPickerState,
) -> Option<i32> {
    const ROW_HEIGHT: f32 = 26.0;
    const WHEEL_SIZE: Vec2 = Vec2::new(76.0, 130.0);

    let unit_index = unit.component_index();
    let label = unit.label();
    let IntervalPickerState {
        component_text: component_values,
        drag,
        scroll_remainder,
    } = state;
    let component_text = &mut component_values[unit_index];
    let mut changed_by = None;
    ui.vertical_centered(|ui| {
        let sense = if enabled {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(WHEEL_SIZE, sense);
        let response = response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if enabled {
                format!("{label}: потяните вверх/вниз, нажмите на соседнее значение или используйте колесо мыши")
            } else {
                "Сначала уменьшите часы, чтобы изменить минуты и секунды".to_owned()
            });
        let painter = ui.painter_at(rect);
        let center = rect.center();
        let selected_rect = egui::Rect::from_center_size(
            center,
            Vec2::new(rect.width() - 8.0, ROW_HEIGHT),
        );
        painter.rect_filled(rect, CornerRadius::same(11), Color32::from_rgb(10, 18, 20));
        painter.rect_stroke(
            rect,
            CornerRadius::same(11),
            Stroke::new(1.0, BORDER.gamma_multiply(if enabled { 0.65 } else { 0.25 })),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            selected_rect,
            CornerRadius::same(7),
            ACCENT.gamma_multiply(if enabled { 0.12 } else { 0.035 }),
        );
        let focused = response.has_focus();
        if focused || response.hovered() {
            painter.rect_stroke(
                selected_rect,
                CornerRadius::same(7),
                Stroke::new(
                    1.0,
                    ACCENT.gamma_multiply(if focused { 0.46 } else { 0.23 }),
                ),
                egui::StrokeKind::Inside,
            );
        }

        let animated_value = ui.ctx().animate_value_with_time(
            response.id.with("wheel-position"),
            value as f32,
            0.12,
        );
        let animated_offset = animated_value - value as f32;
        for offset in -2_i32..=2 {
            let shown_value = value as i32 + offset;
            if !(0..=max_value as i32).contains(&shown_value) {
                continue;
            }
            let y = center.y + (offset as f32 - animated_offset) * ROW_HEIGHT;
            if y < rect.top() - ROW_HEIGHT || y > rect.bottom() + ROW_HEIGHT {
                continue;
            }
            let distance = ((y - center.y).abs() / ROW_HEIGHT).min(3.0);
            let at_center = offset == 0;
            let opacity = (1.0 - distance * 0.30).clamp(0.16, 1.0);
            let color = if !enabled {
                MUTED.gamma_multiply(0.35)
            } else if at_center {
                ACCENT
            } else {
                TEXT.gamma_multiply(opacity)
            };
            if enabled && at_center {
                continue;
            }
            let font_size = if at_center { 22.0 } else { (16.0 - distance * 2.0).max(10.0) };
            painter.text(
                egui::pos2(center.x, y),
                egui::Align2::CENTER_CENTER,
                format!("{shown_value:02}"),
                FontId::proportional(font_size),
                color,
            );
        }

        if enabled {
            if response.clicked() {
                response.request_focus();
                if let Some(pointer) = response.interact_pointer_pos() {
                    let row = ((pointer.y - center.y) / ROW_HEIGHT).round() as i32;
                    if row != 0 {
                        changed_by = Some(row.clamp(-2, 2));
                    }
                }
            }

            if response.drag_started() {
                *drag = Some(IntervalWheelDrag {
                    unit,
                    remainder: 0.0,
                });
                response.request_focus();
            }
            if response.dragged() {
                if let Some(drag_state) = drag.as_mut() {
                    if drag_state.unit == unit {
                        drag_state.remainder -= response.drag_delta().y;
                        let steps = (drag_state.remainder / ROW_HEIGHT).trunc() as i32;
                        drag_state.remainder -= steps as f32 * ROW_HEIGHT;
                        if steps != 0 {
                            changed_by = Some(steps);
                        }
                    }
                }
            }
            if response.drag_stopped() {
                *drag = None;
            }

            if response.has_focus() {
                let (up, down) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                    )
                });
                if up {
                    changed_by = Some(changed_by.unwrap_or(0) - 1);
                }
                if down {
                    changed_by = Some(changed_by.unwrap_or(0) + 1);
                }
            }

            if ui.rect_contains_pointer(rect) {
                let scroll_delta = ui.input(|input| input.smooth_scroll_delta.y);
                if scroll_delta != 0.0 {
                    ui.ctx().input_mut(|input| input.smooth_scroll_delta.y = 0.0);
                    let remainder = match *scroll_remainder {
                        Some((active_unit, remainder)) if active_unit == unit => remainder,
                        _ => 0.0,
                    };
                    let accumulated = remainder + scroll_delta;
                    let steps = -(accumulated / ROW_HEIGHT).trunc() as i32;
                    *scroll_remainder = Some((unit, accumulated + steps as f32 * ROW_HEIGHT));
                    if steps != 0 {
                        changed_by = Some(steps);
                    }
                }
            }
        }

        ui.label(
            RichText::new(label)
                .color(if enabled { MUTED } else { MUTED.gamma_multiply(0.45) })
                .size(9.0),
        );
        if !enabled {
            component_text.clear();
            component_text.push_str(&format!("{value:02}"));
        } else {
            let input_id = ui.make_persistent_id(("interval-component", unit.input_id()));
            let input = ui.put(
                selected_rect.shrink2(Vec2::new(4.0, 1.0)),
                egui::TextEdit::singleline(component_text)
                    .id(input_id)
                    .font(FontId::proportional(20.0))
                    .text_color(ACCENT)
                    .background_color(Color32::TRANSPARENT)
                    .frame(egui::Frame::NONE)
                    .align(egui::Align2::CENTER_CENTER)
                    .margin(egui::Margin::ZERO)
                    .char_limit(2)
                    .return_key(None::<egui::KeyboardShortcut>)
                    .hint_text("00")
                    .desired_width(selected_rect.width() - 8.0),
            );
            let input = input.on_hover_text(format!("Введите {label} в выделенной строке"));
            if input.has_focus() {
                ui.painter().rect_stroke(
                    selected_rect,
                    CornerRadius::same(7),
                    Stroke::new(1.0, ACCENT.gamma_multiply(0.55)),
                    egui::StrokeKind::Inside,
                );
            }
            if !input.has_focus() {
                *component_text = format!("{value:02}");
            }
            if input.changed() {
                if let Ok(entered) = component_text.trim().parse::<u32>() {
                    let adjusted = entered.min(max_value);
                    let delta = adjusted as i32 - value as i32;
                    if entered != adjusted {
                        *component_text = format!("{adjusted:02}");
                    }
                    if delta != 0 {
                        changed_by = Some(delta);
                    }
                    if input.has_focus() && component_text.chars().count() >= 2 {
                        advance_interval_input(ui, input_id, unit);
                    }
                }
            }
            if input.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                advance_interval_input(ui, input_id, unit);
            }
        }
    });
    changed_by
}

fn advance_interval_input(ui: &mut egui::Ui, current_id: egui::Id, unit: IntervalUnit) {
    if let Some(next_unit) = unit.next() {
        let next_id = ui.make_persistent_id(("interval-component", next_unit.input_id()));
        ui.memory_mut(|memory| memory.request_focus(next_id));
    } else {
        ui.memory_mut(|memory| memory.surrender_focus(current_id));
    }
}

fn adjust_interval_component(seconds: u32, unit: IntervalUnit, delta: i32) -> u32 {
    let mut hours = (seconds / 3_600) as i32;
    let mut minutes = ((seconds % 3_600) / 60) as i32;
    let mut remainder = (seconds % 60) as i32;
    match unit {
        IntervalUnit::Hours => hours = (hours + delta).clamp(0, 24),
        IntervalUnit::Minutes => minutes = (minutes + delta).clamp(0, 59),
        IntervalUnit::Seconds => remainder = (remainder + delta).clamp(0, 59),
    }
    (hours * 3_600 + minutes * 60 + remainder).clamp(1, 86_400) as u32
}

fn interval_sound(unit: IntervalUnit, direction: i32) -> &'static str {
    match (unit, direction < 0) {
        (IntervalUnit::Hours, true) => "interval_minus_hour",
        (IntervalUnit::Minutes, true) => "interval_minus_minute",
        (IntervalUnit::Seconds, true) => "interval_minus_second",
        (IntervalUnit::Hours, false) => "interval_plus_hour",
        (IntervalUnit::Minutes, false) => "interval_plus_minute",
        (IntervalUnit::Seconds, false) => "interval_plus_second",
    }
}

fn format_interval(seconds: u32) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let remainder = seconds % 60;
    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours} ч"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes} мин"));
    }
    if remainder > 0 || parts.is_empty() {
        parts.push(format!("{remainder} сек"));
    }
    parts.join(" ")
}

fn format_clock(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        account_remaining, account_switch_remaining, action_changes_active_account,
        adjust_interval_component, format_interval, IntervalUnit,
    };
    use crate::model::{AccountView, UsageSnapshot, UsageWindow};

    #[test]
    fn account_summary_and_switch_threshold_use_their_matching_quota_metrics() {
        let account = AccountView {
            id: "profile".to_owned(),
            account_id: "account".to_owned(),
            email: "person@example.test".to_owned(),
            plan: "Plus".to_owned(),
            created_at: 1,
            last_used_at: None,
            usage: Some(UsageSnapshot {
                primary: Some(UsageWindow {
                    used_percent: 90,
                    window_minutes: Some(300),
                    resets_at: Some(2_000_000_000),
                }),
                secondary: Some(UsageWindow {
                    used_percent: 10,
                    window_minutes: Some(10_080),
                    resets_at: Some(2_000_100_000),
                }),
                updated_at: 1_900_000_000,
                error: None,
            }),
            is_active: true,
        };

        assert_eq!(account_remaining(&account), Some(90));
        assert_eq!(account_switch_remaining(&account), Some(10));
    }

    #[test]
    fn automatic_account_change_is_announced_for_background_updates() {
        assert!(!action_changes_active_account(None));
        assert!(!action_changes_active_account(Some("Обновление лимитов")));
    }

    #[test]
    fn explicit_account_actions_are_not_misreported_as_automatic() {
        assert!(action_changes_active_account(Some("Добавление аккаунта")));
        assert!(action_changes_active_account(Some("Переключение аккаунта")));
        assert!(action_changes_active_account(Some(
            "Импорт резервной копии"
        )));
        assert!(action_changes_active_account(Some("Полный сброс")));
        assert!(action_changes_active_account(Some(
            "Удаление user@example.test"
        )));
    }

    #[test]
    fn interval_picker_formats_and_clamps_the_supported_range() {
        assert_eq!(format_interval(1), "1 сек");
        assert_eq!(format_interval(60), "1 мин");
        assert_eq!(format_interval(3_661), "1 ч 1 мин 1 сек");
        assert_eq!(format_interval(86_400), "24 ч");
        assert_eq!(adjust_interval_component(1, IntervalUnit::Seconds, -1), 1);
        assert_eq!(
            adjust_interval_component(86_400, IntervalUnit::Hours, 1),
            86_400
        );
    }

    #[test]
    fn interval_picker_preserves_other_units_when_editing_one_component() {
        assert_eq!(
            adjust_interval_component(3_661, IntervalUnit::Seconds, 9),
            3_670
        );
        assert_eq!(
            adjust_interval_component(3_661, IntervalUnit::Minutes, -1),
            3_601
        );
        assert_eq!(
            adjust_interval_component(3_661, IntervalUnit::Hours, 1),
            7_261
        );
        assert_eq!(
            adjust_interval_component(86_399, IntervalUnit::Hours, 1),
            86_400
        );
    }
}
