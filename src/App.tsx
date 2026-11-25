import { getLanguage, setLanguage, t, type Language } from "./i18n";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { Effect, getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open, save } from "@tauri-apps/plugin-dialog";
import { AnimatePresence, LayoutGroup, MotionConfig, motion } from "framer-motion";
import {
  Activity, ArrowDownToLine, ArrowRight, Bell, Check, ChevronDown, Clock3,
  CircleHelp, Code2, Command, Columns2, Columns3, Copy, ClipboardPaste, EyeOff, FileKey2, FilePlus2,
  FolderOpen, HardDriveDownload, KeyRound, LoaderCircle, LockKeyhole,
  MessageSquareText, Minus, MoreHorizontal, Plus, RefreshCw, Settings2, Shield,
  Search, SlidersHorizontal, Sun, TerminalSquare, Trash2, Upload, Volume2, VolumeX, Eye,
  ArrowDownWideNarrow, Rows3,
  History, Moon, Route, Waves,
  X, XCircle,
} from "lucide-react";
import type { Account, AccountStatus, AccountStatusKind, AppSettings, CodexPreferences, DashboardData, Prompt, UsageWindow } from "./types";

function languageSwitchHint(target: Language): string {
  return target === "en"
    ? "Переключить интерфейс на английский · Switch interface to English"
    : "Переключить интерфейс на русский · Switch interface to Russian";
}

function accountCountLabel(count: number, language: Language): string {
  if (language === "en") return t(count === 1 ? "аккаунт" : "аккаунтов");
  const lastTwo = count % 100;
  const lastDigit = count % 10;
  if (lastTwo >= 11 && lastTwo <= 14) return t("аккаунтов");
  if (lastDigit === 1) return t("аккаунт");
  if (lastDigit >= 2 && lastDigit <= 4) return t("аккаунта");
  return t("аккаунтов");
}

type Toast = { id: number; title: string; detail?: string; tone: "ok" | "error" | "warning" | "info" };
type LimitAlertData = { accountId: string; email: string; detail: string; resetAt: number | null };
type Modal = "settings" | "codex" | "backup" | "exit" | "reset-settings" | "reset-full" | null;
type ModalOrigin = "settings" | "codex" | "backup" | "auth" | "account" | "center";
type MotionSource = { id: number; left: number; top: number; width: number; height: number } | null;
type ActivityEvent = { id: number; at: number; title: string; detail: string; tone: string };
type LoginAuthInfo = { url?: string | null; userCode?: string | null; deviceCode: boolean; openError?: string | null };
type InstallerSnapshot = { defaultPath: string; selectedPath: string; finish: boolean; uninstall: boolean; appVersion: string };
type TrayMenuAnchor = { side: "top" | "right" | "bottom" | "left"; offsetPercent: number; requestId: number; generation: number };
type AppRestoredEvent = { source?: "tray" | "duplicate" | string };
type AccountColumns = 1 | 2 | 3;
type AccountSort = "remaining" | "reset" | "recent" | "name";

const accountViewStorageKey = "codex-accounts.account-view";

function readAccountView(key: "columns", fallback: AccountColumns): AccountColumns;
function readAccountView(key: "sort", fallback: AccountSort): AccountSort;
function readAccountView(key: "sortReversed", fallback: boolean): boolean;
function readAccountView(key: "columns" | "sort" | "sortReversed", fallback: AccountColumns | AccountSort | boolean): AccountColumns | AccountSort | boolean {
  let value: unknown;
  try {
    const parsed: unknown = JSON.parse(window.localStorage.getItem(accountViewStorageKey) ?? "{}");
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      value = (parsed as Record<string, unknown>)[key];
    }
  } catch {
    return fallback;
  }
  if (key === "columns") {
    return value === 1 || value === 2 || value === 3 ? value : fallback;
  }
  if (key === "sortReversed") return typeof value === "boolean" ? value : fallback;
  return value === "remaining" || value === "reset" || value === "recent" || value === "name"
    ? value
    : fallback;
}

let sharedAudioContext: AudioContext | null = null;
let activeButtonSoundEvent: Event | null = null;
let lastExplicitSoundEvent: Event | null = null;
const buttonAnimationTimers = new WeakMap<HTMLButtonElement, number>();
const pointerBubbleTimers = new WeakMap<HTMLButtonElement, number>();
const quietLifecycleSoundKinds = new Set(["minimize", "hide", "exit", "startup", "restore", "installer-close", "installer-minimize"]);
const quietLifecycleSoundGains: Record<string, number> = {
  minimize: 0.0035,
  hide: 0.0028,
  exit: 0.0022,
  startup: 0.004,
  restore: 0.0034,
  "installer-close": 0.003,
  "installer-minimize": 0.0035,
};
const clearLanguageSoundKinds = new Set(["languageRu", "languageEn", "dashboard-language-ru", "dashboard-language-en", "installer-language-ru", "installer-language-en"]);

function hashLabel(label: string) {
  let hash = 2166136261;
  for (const character of label) hash = Math.imul(hash ^ character.charCodeAt(0), 16777619);
  return hash >>> 0;
}

const initialPreferences: CodexPreferences = {
  model: "", modelReasoningEffort: "", approvalPolicy: "", sandboxMode: "", developerInstructions: "",
};

function activatePointerGlassSurface(surface: HTMLElement | null) {
  if (!surface) return;
  if (surface.matches("button, .model-picker > input")) {
    surface.style.setProperty("--glass-base-image", window.getComputedStyle(surface).backgroundImage || "none");
  }
  surface.classList.add("pointer-glass-active");
}

function deactivatePointerGlassSurface(surface: HTMLElement | null) {
  if (!surface) return;
  surface.classList.remove("pointer-glass-active");
  surface.style.removeProperty("--glass-base-image");
  surface.style.removeProperty("--glass-x");
  surface.style.removeProperty("--glass-y");
  surface.style.removeProperty("--glass-tilt-x");
  surface.style.removeProperty("--glass-tilt-y");
  surface.style.removeProperty("--glass-stretch-x");
  surface.style.removeProperty("--glass-stretch-y");
}

function remaining(window: UsageWindow | null, nowSeconds = Math.floor(Date.now() / 1000)) {
  if (!window) return null;
  if (window.resetsAt !== null && window.resetsAt <= nowSeconds) return 100;
  return Math.max(0, Math.min(100, 100 - window.usedPercent));
}

/** Fit constrained, single-line labels to their actual rendered width. The
 *  hidden DOM measuring node uses the same browser font shaping as the control,
 *  so variable fonts, letter spacing and translated labels are included. */
function useAdaptiveSingleLineFontSize<T extends HTMLElement>(
  ref: React.RefObject<T | null>,
  text: string,
  maxFontSize: number,
  minFontSize = Math.max(8, maxFontSize * 0.75),
) {
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element || !text) return;

    const measureNode = document.createElement("span");
    measureNode.setAttribute("aria-hidden", "true");
    Object.assign(measureNode.style, {
      position: "fixed",
      left: "-10000px",
      top: "0",
      display: "inline-block",
      width: "max-content",
      maxWidth: "none",
      padding: "0",
      margin: "0",
      border: "0",
      whiteSpace: "pre",
      visibility: "hidden",
      pointerEvents: "none",
      contain: "layout style paint",
    });
    document.body.appendChild(measureNode);

    let disposed = false;
    const update = () => {
      if (disposed || !element.isConnected) return;
      const style = window.getComputedStyle(element);
      const horizontalPadding = (Number.parseFloat(style.paddingLeft) || 0)
        + (Number.parseFloat(style.paddingRight) || 0);
      const availableWidth = Math.max(0, element.clientWidth - horizontalPadding - 3);
      if (availableWidth <= 0) return;

      measureNode.textContent = text;
      measureNode.style.fontFamily = style.fontFamily;
      measureNode.style.fontStyle = style.fontStyle;
      measureNode.style.fontWeight = style.fontWeight;
      measureNode.style.fontStretch = style.fontStretch;
      measureNode.style.fontVariant = style.fontVariant;
      measureNode.style.fontVariantCaps = style.fontVariantCaps;
      measureNode.style.fontKerning = style.fontKerning;
      measureNode.style.fontFeatureSettings = style.fontFeatureSettings;
      measureNode.style.fontVariationSettings = style.fontVariationSettings;
      measureNode.style.fontOpticalSizing = style.fontOpticalSizing;
      measureNode.style.fontSizeAdjust = style.fontSizeAdjust;
      measureNode.style.fontSynthesis = style.fontSynthesis;
      measureNode.style.letterSpacing = style.letterSpacing;
      measureNode.style.wordSpacing = style.wordSpacing;
      measureNode.style.textTransform = style.textTransform;
      const measure = (fontSize: number) => {
        measureNode.style.fontSize = `${fontSize}px`;
        return measureNode.getBoundingClientRect().width;
      };

      const lowerBound = Math.min(minFontSize, maxFontSize);
      let fittedSize = maxFontSize;
      if (measure(maxFontSize) > availableWidth) {
        if (measure(lowerBound) > availableWidth) {
          fittedSize = lowerBound;
        } else {
          let low = lowerBound;
          let high = maxFontSize;
          for (let iteration = 0; iteration < 9; iteration += 1) {
            const midpoint = (low + high) / 2;
            if (measure(midpoint) <= availableWidth) low = midpoint;
            else high = midpoint;
          }
          fittedSize = low;
        }
      }

      const cssSize = `${Math.floor(fittedSize * 100) / 100}px`;
      if (element.style.getPropertyValue("--adaptive-fit-font-size") !== cssSize) {
        element.style.setProperty("--adaptive-fit-font-size", cssSize);
      }
    };

    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    observer?.observe(element);
    if (element.parentElement) observer?.observe(element.parentElement);
    void document.fonts?.ready.then(update);
    update();

    return () => {
      disposed = true;
      observer?.disconnect();
      element.style.removeProperty("--adaptive-fit-font-size");
      measureNode.remove();
    };
  }, [ref, text, maxFontSize, minFontSize]);
}

const accountNameCollator = new Intl.Collator("ru");

function sortAccounts(accounts: Account[], mode: AccountSort = "remaining", nowSeconds = Math.floor(Date.now() / 1000), reversed = false) {
  const entries = accounts.map((account) => ({
    account,
    unavailable: permanentAccountStatuses.includes(account.status?.kind ?? "healthy"),
    remaining: bestRemaining(account, nowSeconds),
    reset: nextResetAt(account, nowSeconds) ?? Number.MAX_SAFE_INTEGER,
  }));
  entries.sort((a, b) => {
    if (a.unavailable !== b.unavailable) return a.unavailable ? 1 : -1;
    if ((a.remaining === null) !== (b.remaining === null)) return a.remaining === null ? 1 : -1;
    const quotaDifference = (b.remaining ?? -1) - (a.remaining ?? -1);
    let comparison = 0;
    if (mode === "name") comparison = accountNameCollator.compare(a.account.email, b.account.email);
    else if (mode === "recent") {
      const recent = (b.account.lastUsedAt ?? 0) - (a.account.lastUsedAt ?? 0);
      comparison = recent || quotaDifference;
    }
    else if (mode === "reset") comparison = a.reset - b.reset || quotaDifference;
    else comparison = quotaDifference || a.reset - b.reset;
    return reversed ? -comparison : comparison;
  });
  return entries.map(({ account }) => account);
}

function bestRemaining(account: Account, nowSeconds = Math.floor(Date.now() / 1000)) {
  const values = [remaining(account.usage?.primary ?? null, nowSeconds), remaining(account.usage?.secondary ?? null, nowSeconds)].filter((value): value is number => value !== null);
  return values.length ? Math.max(...values) : null;
}

function availableNow(account: Account, nowSeconds = Math.floor(Date.now() / 1000)) {
  const values = [remaining(account.usage?.primary ?? null, nowSeconds), remaining(account.usage?.secondary ?? null, nowSeconds)].filter((value): value is number => value !== null);
  return values.length ? Math.min(...values) : null;
}

function matchesAccountSearch(account: Account, query: string) {
  const needle = query.trim().toLocaleLowerCase("ru-RU");
  if (!needle) return true;
  return [account.email, account.email.split("@")[0], account.accountId, account.plan]
    .some((value) => value.toLocaleLowerCase("ru-RU").includes(needle));
}

function nextResetAt(account: Account, nowSeconds = Math.floor(Date.now() / 1000)) {
  return [account.usage?.primary, account.usage?.secondary]
    .filter((window): window is UsageWindow => !!window && remaining(window, nowSeconds) !== 100 && window.resetsAt !== null)
    .map((window) => window.resetsAt as number)
    .sort((a, b) => a - b)[0] ?? null;
}

const soundTracks: Record<string, [number, number, number][]> = {
    refresh: [[659, 0, 0.09], [880, 0.1, 0.13]], backup: [[523, 0, 0.1], [698, 0.11, 0.1], [1047, 0.23, 0.14]],
    settings: [[587, 0, 0.11], [784, 0.13, 0.14]], codex: [[440, 0, 0.11], [587, 0.12, 0.1], [784, 0.24, 0.15]], connect: [[349, 0, 0.1], [523, 0.11, 0.1], [698, 0.23, 0.14]], disconnect: [[698, 0, 0.08], [523, 0.1, 0.09], [392, 0.21, 0.16]], codexSave: [[587, 0, 0.07], [740, 0.1, 0.08], [988, 0.2, 0.14]],
    settingsSave: [[622, 0, 0.07], [831, 0.09, 0.09], [932, 0.2, 0.15]], securitySave: [[392, 0, 0.1], [587, 0.11, 0.09], [784, 0.23, 0.17]],
    soundOn: [[784, 0, 0.08], [988, 0.09, 0.15]], soundOff: [[988, 0, 0.08], [659, 0.1, 0.16]],
    add: [[523, 0, 0.08], [659, 0.08, 0.1], [784, 0.17, 0.16]], accountAdded: [[392, 0, 0.07], [587, 0.08, 0.1], [784, 0.19, 0.18]], cancel: [[523, 0, 0.08], [392, 0.11, 0.14]], authCopy: [[831, 0, 0.07], [1047, 0.08, 0.12]], authPaste: [[587, 0, 0.07], [784, 0.09, 0.08], [988, 0.19, 0.12]], authPrivate: [[659, 0, 0.08], [523, 0.09, 0.07], [784, 0.18, 0.13]], theme: [[740, 0, 0.08], [932, 0.1, 0.11]], "dashboard-theme": [[740, 0, 0.07], [932, 0.09, 0.08], [1245, 0.21, 0.13]], languageRu: [[392, 0, 0.07], [587, 0.08, 0.08], [784, 0.18, 0.12]], languageEn: [[523, 0, 0.07], [698, 0.09, 0.08], [988, 0.18, 0.12]], effects: [[440, 0, 0.08], [554, 0.1, 0.08], [740, 0.2, 0.13]], switch: [[740, 0, 0.1], [587, 0.1, 0.08], [880, 0.2, 0.15]],
    autoSwitch: [[494, 0, 0.08], [659, 0.1, 0.08], [988, 0.22, 0.16]],
    update: [[659, 0, 0.08], [784, 0.1, 0.1]], success: [[587, 0, 0.08], [740, 0.08, 0.09], [988, 0.18, 0.13]],
    resetSettings: [[523, 0, 0.12], [659, 0.14, 0.1], [587, 0.27, 0.16]], resetAll: [[440, 0, 0.13], [523, 0.16, 0.1], [392, 0.3, 0.2]],
    promptAdd: [[392, 0, 0.08], [587, 0.09, 0.09], [698, 0.2, 0.14]], promptSave: [[494, 0, 0.08], [659, 0.09, 0.09], [880, 0.2, 0.15]],
    error: [[493, 0, 0.12], [392, 0.15, 0.12], [440, 0.31, 0.19]], deletePrompt: [[494, 0, 0.08], [392, 0.1, 0.08], [330, 0.2, 0.15]], accountDelete: [[392, 0, 0.09], [330, 0.11, 0.1], [262, 0.24, 0.17]], promptDelete: [[659, 0, 0.07], [494, 0.1, 0.08], [370, 0.2, 0.14]], delete: [[440, 0, 0.08], [349, 0.1, 0.15]],
    minimize: [[740, 0, 0.055], [988, 0.075, 0.075]], hide: [[494, 0, 0.06], [370, 0.095, 0.08]], expand: [[523, 0, 0.08], [659, 0.08, 0.08], [880, 0.16, 0.12]], startup: [[392, 0, 0.09], [523, 0.1, 0.08], [659, 0.21, 0.16]],
    restore: [[370, 0, 0.08], [554, 0.09, 0.08], [740, 0.19, 0.14]], exitPrompt: [[587, 0, 0.08], [494, 0.12, 0.1]], notice: [[698, 0, 0.07], [880, 0.1, 0.12]],
    prompt: [[587, 0, 0.08], [740, 0.1, 0.08], [587, 0.2, 0.12]], unlock: [[330, 0, 0.08], [494, 0.1, 0.09], [659, 0.21, 0.15]], backupExport: [[784, 0, 0.08], [988, 0.1, 0.09], [1319, 0.21, 0.14]], backupImport: [[988, 0, 0.08], [784, 0.1, 0.09], [659, 0.21, 0.14]],
    exit: [[392, 0, 0.07], [311, 0.095, 0.07], [262, 0.19, 0.1]],
    "installer-startup": [[294, 0, 0.08], [370, 0.09, 0.08], [587, 0.2, 0.18]],
    "installer-close": [[784, 0, 0.07], [659, 0.09, 0.08], [523, 0.2, 0.14]],
    "installer-prepare": [[330, 0, 0.08], [440, 0.1, 0.08], [554, 0.2, 0.16]],
    "installer-prepare-error": [[440, 0, 0.11], [349, 0.13, 0.1], [262, 0.28, 0.18]],
    "installer-finish": [[523, 0, 0.07], [659, 0.08, 0.08], [1047, 0.19, 0.18]],
    "installer-launch": [[659, 0, 0.07], [784, 0.09, 0.08], [1175, 0.2, 0.16]],
    "installer-launch-error": [[294, 0, 0.12], [247, 0.14, 0.1], [196, 0.29, 0.2]],
    "installer-browse": [[587, 0, 0.07], [740, 0.09, 0.08], [988, 0.2, 0.13]],
    "installer-browse-open": [[416, 0, 0.09], [554, 0.075, 0.1]],
    "installer-browse-error": [[440, 0, 0.11], [330, 0.11, 0.13]],
    "installer-open-folder": [[440, 0, 0.08], [554, 0.08, 0.09], [740, 0.15, 0.12]],
    "installer-open-folder-error": [[523, 0, 0.08], [415, 0.09, 0.1], [330, 0.17, 0.13]],
    "installer-uninstall": [[659, 0, 0.09], [440, 0.09, 0.1], [330, 0.19, 0.14]],
    "installer-uninstall-finish": [[330, 0, 0.09], [494, 0.08, 0.08], [659, 0.16, 0.16]],
    "installer-theme": [[494, 0, 0.08], [622, 0.1, 0.08], [831, 0.21, 0.14]],
    "installer-language-ru": [[392, 0, 0.07], [494, 0.09, 0.08], [659, 0.2, 0.14]],
    "installer-language-en": [[523, 0, 0.07], [659, 0.09, 0.08], [880, 0.2, 0.14]],
    "installer-sound-on": [[440, 0, 0.07], [659, 0.09, 0.08], [988, 0.2, 0.14]],
    "installer-sound-off": [[988, 0, 0.07], [659, 0.1, 0.08], [440, 0.21, 0.14]],
    "installer-effects-on": [[370, 0, 0.07], [554, 0.09, 0.08], [831, 0.2, 0.16]],
    "installer-effects-off": [[831, 0, 0.07], [554, 0.1, 0.08], [370, 0.21, 0.16]],
    click: [[784, 0, 0.045]],
};

let glassWave: PeriodicWave | null = null;

function sound(kind: string, enabled: boolean, force = false) {
  if (!enabled && !force) return;
  if (activeButtonSoundEvent) lastExplicitSoundEvent = activeButtonSoundEvent;
  const AudioContextClass = window.AudioContext || (window as Window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!AudioContextClass) return;
  const notes = soundTracks[kind] ?? soundMotif(kind);
  try {
    const context = sharedAudioContext ?? (sharedAudioContext = new AudioContextClass());
    if (context.state === "suspended") void context.resume().catch(() => undefined);
    glassWave ??= context.createPeriodicWave(new Float32Array(4), new Float32Array([0, 1, .105, .025]));
    const startedAt = context.currentTime + .008;
    const master = context.createGain();
    master.gain.value = clearLanguageSoundKinds.has(kind)
      ? 0.029
      : kind === "expand"
        ? 0.025
      : quietLifecycleSoundKinds.has(kind)
        ? quietLifecycleSoundGains[kind] ?? 0.007
        : 0.03;
    master.connect(context.destination);
    let activeNotes = notes.length;
    for (const [frequency, start, length] of notes) {
      const noteStart = startedAt + start;
      const oscillator = context.createOscillator();
      const envelope = context.createGain();
      oscillator.setPeriodicWave(glassWave);
      oscillator.frequency.value = frequency;
      envelope.gain.setValueAtTime(0.0001, noteStart);
      envelope.gain.exponentialRampToValueAtTime(0.30, noteStart + 0.016);
      envelope.gain.exponentialRampToValueAtTime(0.0001, noteStart + length);
      oscillator.connect(envelope).connect(master);
      oscillator.onended = () => {
        oscillator.disconnect();
        envelope.disconnect();
        activeNotes -= 1;
        if (activeNotes === 0) master.disconnect();
      };
      oscillator.start(noteStart);
      oscillator.stop(noteStart + length + 0.008);
    }
  } catch { /* Audio remains optional when the Windows device is unavailable. */ }
}

async function writeClipboardText(value: string) {
  if (!value.trim()) return false;
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(value);
      return true;
    } catch {
      // Tauri/WebView2 can reject clipboard permission until a document-level
      // selection has been made.  Keep the old synchronous fallback below.
    }
  }
  const temporary = document.createElement("textarea");
  temporary.value = value;
  temporary.setAttribute("readonly", "true");
  temporary.style.position = "fixed";
  temporary.style.inset = "-9999px auto auto -9999px";
  temporary.style.opacity = "0";
  document.body.appendChild(temporary);
  temporary.focus();
  temporary.select();
  let copied = false;
  try {
    copied = document.execCommand("copy");
  } catch {
    copied = false;
  }
  temporary.remove();
  return copied;
}

function soundMotif(key: string): [number, number, number][] {
  const hash = hashLabel(key);
  const scale = [0, 2, 4, 5, 7, 9, 12, 14];
  const root = 392 * 2 ** (((hash % 13) - 6) / 12);
  const note = (index: number) => root * 2 ** (scale[index % scale.length] / 12);
  const rhythm = [
    [0, 0.045, 0.105],
    [0, 0.065, 0.11],
    [0, 0.055, 0.14],
    [0, 0.085, 0.135],
  ][(hash >>> 24) % 4];
  return [
    [root, rhythm[0], 0.05],
    [note(hash >>> 5), rhythm[1], 0.055],
    [note(hash >>> 11), rhythm[2], 0.06],
  ];
}

function useBrandedTooltips() {
  useEffect(() => {
    // Settings and account dialogs render through createPortal(document.body),
    // so delegated hints must watch the body instead of only the React root.
    const host = document.body;
    const tooltip = document.createElement("div");
    tooltip.className = "global-tooltip theme-dark";
    tooltip.setAttribute("role", "tooltip");
    tooltip.hidden = true;
    const content = document.createElement("span");
    const arrow = document.createElement("i");
    arrow.setAttribute("aria-hidden", "true");
    tooltip.append(content, arrow);
    document.body.appendChild(tooltip);

    let active: HTMLElement | null = null;
    let showTimer = 0;
    let positionFrame = 0;

    const findTarget = (target: EventTarget | null) => {
      // SVG paths are frequent pointer targets inside icon buttons. Element
      // also covers them, so the branded tooltip resolves to the button.
      return target instanceof Element ? target.closest<HTMLElement>("[data-tooltip]") : null;
    };

    const hide = () => {
      window.clearTimeout(showTimer);
      active = null;
      tooltip.classList.remove("is-visible", "is-below");
      tooltip.hidden = true;
    };

    const position = () => {
      positionFrame = 0;
      if (!active || !active.isConnected) {
        hide();
        return;
      }
      const text = active.getAttribute("data-tooltip")?.trim();
      if (!text) {
        hide();
        return;
      }
      content.textContent = text;
      tooltip.classList.toggle("theme-light", !!active.closest(".theme-light"));
      tooltip.classList.toggle("theme-dark", !active.closest(".theme-light"));
      tooltip.hidden = false;
      tooltip.style.left = "0px";
      tooltip.style.top = "0px";
      const targetRect = active.getBoundingClientRect();
      const targetStyle = window.getComputedStyle(active);
      if (!targetRect.width || !targetRect.height || targetStyle.display === "none" || targetStyle.visibility === "hidden" || Number.parseFloat(targetStyle.opacity || "1") < 0.02) {
        hide();
        return;
      }
      const bubbleRect = tooltip.getBoundingClientRect();
      const margin = 10;
      const centeredLeft = targetRect.left + targetRect.width / 2 - bubbleRect.width / 2;
      const left = Math.max(margin, Math.min(centeredLeft, window.innerWidth - bubbleRect.width - margin));
      const canSitAbove = targetRect.top - bubbleRect.height - 10 >= margin;
      const rawTop = canSitAbove ? targetRect.top - bubbleRect.height - 9 : targetRect.bottom + 9;
      const top = Math.max(margin, Math.min(rawTop, window.innerHeight - bubbleRect.height - margin));
      const arrowLeft = Math.max(13, Math.min(bubbleRect.width - 13, targetRect.left + targetRect.width / 2 - left));
      tooltip.classList.toggle("is-below", !canSitAbove);
      tooltip.style.left = `${Math.round(left)}px`;
      tooltip.style.top = `${Math.round(top)}px`;
      tooltip.style.setProperty("--tooltip-arrow-x", `${Math.round(arrowLeft)}px`);
      tooltip.classList.add("is-visible");
    };

    const schedulePosition = () => {
      if (!positionFrame) positionFrame = window.requestAnimationFrame(position);
    };

    const show = (element: HTMLElement) => {
      const text = element.getAttribute("data-tooltip")?.trim();
      if (!text) return;
      window.clearTimeout(showTimer);
      active = element;
      showTimer = window.setTimeout(schedulePosition, 45);
    };

    const migrateTitles = () => {
      // React can remove a hovered control while its modal or toast is
      // closing.  In that case pointerout is not guaranteed to fire, so do
      // not leave the detached tooltip floating over the next surface.
      if (active && !active.isConnected) {
        hide();
      }
      host.querySelectorAll<HTMLElement>("button,[role='button'],[role='switch']").forEach((element) => {
        // Keep authored hints intact, but refresh automatically generated hints
        // when React changes an aria-label (language, effects, sound, or theme).
        const existingTooltip = element.getAttribute("data-tooltip");
        if (existingTooltip && element.getAttribute("data-tooltip-auto") !== "true") return;
        const title = element.getAttribute("title")?.trim();
        const label = element.getAttribute("aria-label")?.trim();
        const tooltipCopy = element.querySelector<HTMLElement>(".choice-picker-option-copy, .account-sort-option-copy");
        const structuredText = tooltipCopy
          ? Array.from(tooltipCopy.children).map((part) => part.textContent?.replace(/\s+/g, " ").trim() ?? "").filter(Boolean).join(" — ")
          : "";
        const text = structuredText || element.textContent?.replace(/\s+/g, " ").trim();
        const nextTooltip = title || label || (text && text.length <= 96 ? text : "");
        if (!nextTooltip) return;
        if (existingTooltip !== nextTooltip) element.setAttribute("data-tooltip", nextTooltip);
        element.setAttribute("data-tooltip-auto", "true");
        if (title) element.removeAttribute("title");
      });
      if (active) schedulePosition();
    };

    const onPointerOver = (event: PointerEvent) => {
      const target = findTarget(event.target);
      if (!target) return;
      const related = event.relatedTarget;
      if (related instanceof Node && target.contains(related)) return;
      show(target);
    };
    const onPointerOut = (event: PointerEvent) => {
      const target = findTarget(event.target);
      if (!target) return;
      const related = event.relatedTarget;
      if (related instanceof Node && target.contains(related)) return;
      if (target === active) hide();
    };
    const onFocusIn = (event: FocusEvent) => {
      const target = findTarget(event.target);
      if (target) show(target);
    };
    const onFocusOut = (event: FocusEvent) => {
      const target = findTarget(event.target);
      if (target && target === active) hide();
    };
    const onPointerDown = () => {
      // Clicking a control should consume its hint immediately.  This keeps
      // a tooltip from surviving a route/modal change when the pointer stays
      // at the same coordinates.
      if (active) hide();
    };
    const onDocumentInteraction = () => {
      if (active) hide();
    };

    migrateTitles();
    const observer = new MutationObserver(migrateTitles);
    observer.observe(host, {
      subtree: true,
      childList: true,
      characterData: true,
      attributes: true,
      attributeFilter: ["title", "aria-label"],
    });
    host.addEventListener("pointerover", onPointerOver, true);
    host.addEventListener("pointerout", onPointerOut, true);
    host.addEventListener("pointerdown", onPointerDown, true);
    host.addEventListener("focusin", onFocusIn, true);
    host.addEventListener("focusout", onFocusOut, true);
    document.addEventListener("mousedown", onDocumentInteraction, true);
    document.addEventListener("click", onDocumentInteraction, true);
    window.addEventListener("resize", schedulePosition, { passive: true });
    window.addEventListener("scroll", schedulePosition, true);

    return () => {
      observer.disconnect();
      host.removeEventListener("pointerover", onPointerOver, true);
      host.removeEventListener("pointerout", onPointerOut, true);
      host.removeEventListener("pointerdown", onPointerDown, true);
      host.removeEventListener("focusin", onFocusIn, true);
      host.removeEventListener("focusout", onFocusOut, true);
      document.removeEventListener("mousedown", onDocumentInteraction, true);
      document.removeEventListener("click", onDocumentInteraction, true);
      window.removeEventListener("resize", schedulePosition);
      window.removeEventListener("scroll", schedulePosition, true);
      window.clearTimeout(showTimer);
      if (positionFrame) window.cancelAnimationFrame(positionFrame);
      tooltip.remove();
    };
  }, []);
}

function buttonActionKey(button: HTMLButtonElement) {
  const explicit = button.dataset.feedback || button.dataset.sound?.trim();
  if (explicit) return explicit;
  const label = button.getAttribute("aria-label") || button.title || button.textContent?.trim() || "button";
  const scope = button.className.split(/\s+/).filter(Boolean).slice(0, 3).join(":");
  return `${scope}:${label}`;
}

function usePrefersReducedMotion() {
  const [reduced, setReduced] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReduced(media.matches);
    update();
    media.addEventListener?.("change", update);
    return () => media.removeEventListener?.("change", update);
  }, []);
  return reduced;
}

/**
 * Measure the compositor cadence once when the shell mounts.  WebView2 already
 * schedules CSS and Framer Motion frames at the monitor cadence; exposing the
 * measured value as a token lets the small CSS transitions keep their timing
 * proportional to a 60/120/144/180 Hz display without creating a second timer
 * loop for every component.
 */
function useDisplayRefreshRate() {
  const [refreshRate, setRefreshRate] = useState(60);
  useEffect(() => {
    let frame = 0;
    let timer = 0;
    let disposed = false;
    const measure = () => {
      if (disposed || document.visibilityState === "hidden") return;
      let frames = 0;
      const startedAt = performance.now();
      const sample = (now: number) => {
        if (disposed) return;
        frames += 1;
        const elapsed = now - startedAt;
        if (elapsed >= 420) {
          const measured = Math.round((frames * 1000) / elapsed);
          setRefreshRate(Math.max(30, Math.min(240, Number.isFinite(measured) ? measured : 60)));
          return;
        }
        frame = window.requestAnimationFrame(sample);
      };
      frame = window.requestAnimationFrame(sample);
    };
    const scheduleMeasure = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(measure, 180);
    };
    const onVisibility = () => {
      if (document.visibilityState === "visible") scheduleMeasure();
    };
    measure();
    window.addEventListener("resize", scheduleMeasure, { passive: true });
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      disposed = true;
      window.cancelAnimationFrame(frame);
      window.clearTimeout(timer);
      window.removeEventListener("resize", scheduleMeasure);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);
  useEffect(() => {
    document.documentElement.style.setProperty("--display-hz", String(refreshRate));
  }, [refreshRate]);
  return refreshRate;
}

function playUnassignedButtonSound(event: React.MouseEvent, enabled: boolean) {
  const element = event.target instanceof Element ? event.target : null;
  const button = element?.closest("button");
  if (!button || button.disabled || lastExplicitSoundEvent === event.nativeEvent) return;
  sound(`button:${buttonActionKey(button)}`, enabled);
}

/** Start the shared bubble response on the press edge, before React applies the
 * next toggle state.  The click handler still owns the sound acknowledgement,
 * while keyboard activation continues to work through the click path. */
function animateButtonElement(button: HTMLButtonElement) {
  if (button.disabled || !button.isConnected) return;
  const isEffectsControl = button.classList.contains("effects-toggle") || button.classList.contains("installer-effects-toggle");
  if (button.closest(".effects-off") && !isEffectsControl) return;
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const label = buttonActionKey(button);
  const actionHash = hashLabel(label);
  const bubbleVariant = actionHash % 9;
  button.style.setProperty("--bubble-radius", getComputedStyle(button).borderRadius);
  button.dataset.bubbleVariant = String(bubbleVariant);
  button.style.setProperty("--bubble-animation-name", `action-bubble-${bubbleVariant}`);
  const angle = ((actionHash % 7) - 3) * .55;
  button.style.setProperty("--bubble-angle", `${angle}deg`);
  button.style.setProperty("--bubble-return-angle", `${-angle * 0.35}deg`);
  const bubbleX = ((actionHash >>> 4) % 5) - 2;
  const bubbleY = ((actionHash >>> 8) % 3) - 1;
  button.style.setProperty("--bubble-x", `${bubbleX}px`);
  button.style.setProperty("--bubble-y", `${bubbleY}px`);
  button.style.setProperty("--bubble-return-x", `${-bubbleX * 0.35}px`);
  button.style.setProperty("--bubble-return-y", `${-bubbleY * 0.35}px`);
  button.style.setProperty("--bubble-duration", `${255 + (actionHash % 7) * 12}ms`);
  if (button.classList.contains("button-bubble")) {
    for (const animation of button.getAnimations()) {
      if (animation instanceof CSSAnimation && animation.animationName.startsWith("action-bubble-")) {
        animation.currentTime = 0;
        animation.play();
      }
    }
  }
  button.classList.add("button-bubble");
  const previousTimer = buttonAnimationTimers.get(button);
  if (previousTimer !== undefined) window.clearTimeout(previousTimer);
  const timer = window.setTimeout(() => {
    buttonAnimationTimers.delete(button);
    button.classList.remove("button-bubble");
    delete button.dataset.bubbleVariant;
    button.style.removeProperty("--bubble-radius");
    button.style.removeProperty("--bubble-angle");
    button.style.removeProperty("--bubble-return-angle");
    button.style.removeProperty("--bubble-x");
    button.style.removeProperty("--bubble-y");
    button.style.removeProperty("--bubble-return-x");
    button.style.removeProperty("--bubble-return-y");
    button.style.removeProperty("--bubble-duration");
    button.style.removeProperty("--bubble-animation-name");
  }, 370);
  buttonAnimationTimers.set(button, timer);
}

function beginButtonAnimationEvent(event: React.SyntheticEvent) {
  const element = event.target instanceof Element ? event.target : null;
  const button = element?.closest<HTMLButtonElement>("button");
  if (!button || button.disabled || !button.isConnected) return;
  // Pointer down starts the visible response immediately.  Skip the second
  // restart on the following click, but leave keyboard clicks animated.
  if (event.type === "pointerdown") {
    const pointer = event as React.PointerEvent;
    if (!pointer.isPrimary || pointer.button !== 0) return;
    button.dataset.pointerBubble = "1";
    const previousTimer = pointerBubbleTimers.get(button);
    if (previousTimer !== undefined) window.clearTimeout(previousTimer);
    const timer = window.setTimeout(() => {
      pointerBubbleTimers.delete(button);
      if (button.dataset.pointerBubble === "1") delete button.dataset.pointerBubble;
    }, 520);
    pointerBubbleTimers.set(button, timer);
    animateButtonElement(button);
    return;
  }
  if (button.dataset.pointerBubble === "1") {
    delete button.dataset.pointerBubble;
    return;
  }
  animateButtonElement(button);
}

function beginButtonSoundEvent(event: React.MouseEvent) {
  activeButtonSoundEvent = event.nativeEvent;
  beginButtonAnimationEvent(event);
}

function finishButtonSoundEvent(event: React.MouseEvent, enabled: boolean) {
  playUnassignedButtonSound(event, enabled);
  activeButtonSoundEvent = null;
}

function markActiveButtonSoundEventHandled() {
  if (activeButtonSoundEvent) lastExplicitSoundEvent = activeButtonSoundEvent;
}

function formatTime(timestamp: number | null | undefined) {
  if (!timestamp) return t("неизвестно");
  const locale = getLanguage() === "en" ? "en-US" : "ru-RU";
  return new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" }).format(new Date(timestamp * 1000));
}

function EmailText({ email }: { email: string }) {
  const parts = email.split("@");
  return <>{parts.map((part, index) => <span key={`${part}-${index}`}>{index > 0 ? "@" : ""}{part}{index < parts.length - 1 && <wbr />}</span>)}</>;
}

function formatReset(timestamp: number | null | undefined) {
  if (!timestamp) return t("неизвестно");
  const date = new Date(timestamp * 1000);
  const today = new Date();
  if (date.toDateString() === today.toDateString()) return `${t("сегодня")}, ${formatTime(timestamp)}`;
  // Keep the date compact. Native Russian short dates add a trailing full
  // stop ("нояб.") and can clip a quota row in the three-column layout.
  const day = date.getDate();
  const month = getLanguage() === "en"
    ? ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][date.getMonth()]
    : ["янв", "фев", "мар", "апр", "май", "июн", "июл", "авг", "сен", "окт", "ноя", "дек"][date.getMonth()];
  const dayMonth = getLanguage() === "en" ? `${month} ${day}` : `${day} ${month}`;
  return `${dayMonth} · ${formatTime(timestamp)}`;
}

function formatUpdated(timestamp: number | null | undefined) {
  if (!timestamp) return t("Ожидает первой проверки");
  const minutes = Math.max(0, Math.floor((Date.now() / 1000 - timestamp) / 60));
  if (minutes < 1) return t("только что");
  if (minutes === 1) return t("минуту назад");
  if (minutes < 60) return getLanguage() === "en" ? `${minutes} min ago` : `${minutes} мин назад`;
  return getLanguage() === "en" ? `${Math.floor(minutes / 60)} hr ago` : `${Math.floor(minutes / 60)} ч назад`;
}

function formatInterval(seconds: number) {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainder = seconds % 60;
  const parts = [
    hours ? `${hours} ${t("ч")}` : "",
    minutes ? `${minutes} ${t("мин")}` : "",
    remainder || (!hours && !minutes) ? `${remainder} ${t("сек")}` : "",
  ].filter(Boolean);
  return parts.join(" ");
}

function isFreePlan(plan: string) {
  return plan.trim().toLocaleLowerCase("en-US") === "free";
}

function displayPlan(plan: string) {
  return isFreePlan(plan) ? "FREE" : plan;
}

function formatWindowLabel(minutes: number | null | undefined) {
  if (!minutes) return t("окно лимита");
  if (minutes >= 10000) return t("недельный лимит");
  if (minutes >= 1200) return t("дневной лимит");
  if (minutes >= 240) return t("5‑часовой лимит");
  if (minutes >= 60) return t(`${Math.round(minutes / 60)}‑часовой лимит`);
  return t(`${minutes}‑минутный лимит`);
}

function toneFor(value: number | null) {
  if (value === null) return "unknown";
  if (value <= 5) return "critical";
  if (value <= 25) return "low";
  return "healthy";
}

const permanentAccountStatuses: AccountStatusKind[] = ["reauthRequired", "accessDenied", "banned", "corruptAuth"];

function accountStatusInfo(status: AccountStatus | undefined) {
  const kind = status?.kind ?? "healthy";
  const info: Record<AccountStatusKind, { label: string; hint: string; tone: "ok" | "warning" | "error"; action: string }> = {
    healthy: { label: "Работает", hint: "Вход действителен.", tone: "ok", action: "" },
    usageLimit: { label: "Лимит исчерпан", hint: "Профиль вернётся после сброса окна.", tone: "warning", action: "" },
    reauthRequired: { label: "Нужен повторный вход", hint: "Войдите заново через официальный браузерный вход.", tone: "error", action: "Войти заново" },
    accessDenied: { label: "Доступ запрещён", hint: "Проверьте план и права аккаунта; при необходимости выберите другой профиль вручную.", tone: "error", action: "" },
    banned: { label: "Аккаунт заблокирован", hint: "Проверьте письмо OpenAI или обратитесь в поддержку. Для работы выберите другой профиль вручную.", tone: "error", action: "" },
    corruptAuth: { label: "Вход повреждён", hint: "Удалите профиль и добавьте его заново через официальный вход.", tone: "error", action: "Войти заново" },
    temporaryError: { label: "Проверка временно недоступна", hint: "Проверьте интернет и повторите обновление.", tone: "warning", action: "" },
  };
  const selected = info[kind];
  return {
    ...selected,
    label: t(selected.label),
    hint: t(selected.hint),
    action: selected.action ? t(selected.action) : "",
    kind,
    detail: status?.detail ? t(status.detail) : null,
    permanent: permanentAccountStatuses.includes(kind),
  };
}

function displayAccountStatus(account: Account, nowSeconds: number) {
  const quotaRemaining = availableNow(account, nowSeconds);
  const resetCompleted = account.status.kind === "usageLimit" && quotaRemaining !== null && quotaRemaining > 0;
  return accountStatusInfo(resetCompleted ? { kind: "healthy", detail: null } : account.status);
}

function manualSwitchBlockReason(account: Account, nowSeconds: number) {
  const status = displayAccountStatus(account, nowSeconds);
  if (status.permanent) return status.hint;
  const quota = availableNow(account, nowSeconds);
  if (quota === 0) return "У выбранного профиля исчерпано одно из окон лимита. Обновите данные или дождитесь сброса.";
  if (account.status.kind === "usageLimit" && quota === null) return "Остаток лимита этого профиля неизвестен. Обновите данные перед переключением.";
  return null;
}

function getErrorText(error: unknown) {
  if (typeof error === "string") return t(error);
  if (error instanceof Error) return t(error.message);
  return t("Действие не выполнено. Проверьте подключение и повторите.");
}

function toastEntryMotion(tone: Toast["tone"]) {
  if (tone === "error") return { opacity: 0, x: 18, scale: .97, rotate: .35 };
  if (tone === "warning") return { opacity: 0, y: 16, scale: .94, rotate: -.45 };
  if (tone === "info") return { opacity: 0, x: -16, y: 4, scale: .98 };
  return { opacity: 0, y: 22, scale: .91, rotate: -.3 };
}

function toastExitMotion(tone: Toast["tone"]) {
  if (tone === "error") return { opacity: 0, x: 14, scale: .97 };
  if (tone === "warning") return { opacity: 0, y: -8, scale: .985 };
  if (tone === "info") return { opacity: 0, x: 12, scale: .985 };
  return { opacity: 0, y: 10, scale: .97 };
}

function OriginMotionSection({ source, kind, animate, className, children, role = "dialog", labelledBy, onDismiss }: {
  source: MotionSource;
  kind: ModalOrigin;
  animate: boolean;
  className: string;
  children: React.ReactNode;
  role?: "dialog" | "alertdialog";
  labelledBy?: string;
  onDismiss: () => void;
}) {
  const sheetRef = useRef<HTMLElement | null>(null);
  const [pose, setPose] = useState({ x: 0, y: 18, scale: .94, originX: "50%", originY: "50%", ready: false });

  useLayoutEffect(() => {
    const previous = document.activeElement;
    return () => {
      queueMicrotask(() => {
        if (previous instanceof HTMLElement && previous.isConnected && document.activeElement === document.body) {
          previous.focus({ preventScroll: true });
        }
      });
    };
  }, []);

  useLayoutEffect(() => {
    const sheet = sheetRef.current;
    if (!pose.ready || !sheet) return;
    const focusSheet = () => sheet.focus({ preventScroll: true });
    focusSheet();
    const observer = new MutationObserver(() => {
      // Advancing a confirmation or removing a prompt can unmount its button.
      if (document.hasFocus() && document.activeElement === document.body) focusSheet();
    });
    observer.observe(sheet, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [pose.ready]);

  const handleKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onDismiss();
      return;
    }
    if (event.key !== "Tab") return;
    const sheet = event.currentTarget;
    const controls = Array.from(sheet.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), a[href], [tabindex]',
    )).filter((control) => control.tabIndex >= 0 && control.getClientRects().length > 0);
    const first = controls[0];
    const last = controls[controls.length - 1];
    const focused = document.activeElement;
    if (!first || focused === sheet || (event.shiftKey ? focused === first : focused === last)) {
      event.preventDefault();
      (event.shiftKey ? last : first)?.focus();
    }
  };

  useLayoutEffect(() => {
    const sheet = sheetRef.current;
    if (!sheet) return;
    if (!animate) {
      setPose({ x: 0, y: 0, scale: 1, originX: "50%", originY: "50%", ready: true });
      return;
    }
    const sheetRect = sheet.getBoundingClientRect();
    if (!source || sheetRect.width <= 0 || sheetRect.height <= 0) {
      setPose({ x: 0, y: 20, scale: .94, originX: "50%", originY: "50%", ready: true });
      return;
    }
    const x = source.left + source.width / 2 - (sheetRect.left + sheetRect.width / 2);
    const y = source.top + source.height / 2 - (sheetRect.top + sheetRect.height / 2);
    const scale = Math.max(.07, Math.min(.24, Math.max(source.width / sheetRect.width, source.height / sheetRect.height)));
    const originX = `${Math.max(0, Math.min(100, ((source.left + source.width / 2 - sheetRect.left) / sheetRect.width) * 100))}%`;
    const originY = `${Math.max(0, Math.min(100, ((source.top + source.height / 2 - sheetRect.top) / sheetRect.height) * 100))}%`;
    setPose({ x, y, scale, originX, originY, ready: true });
  }, [animate, source]);

  const spring = kind === "settings"
    ? { type: "spring" as const, stiffness: 285, damping: 25, mass: .82 }
    : kind === "codex"
      ? { type: "spring" as const, stiffness: 330, damping: 29, mass: .74 }
      : kind === "backup"
        ? { type: "spring" as const, stiffness: 305, damping: 24, mass: .78 }
        : kind === "account"
          ? { type: "spring" as const, stiffness: 370, damping: 28, mass: .68 }
          : { type: "spring" as const, stiffness: 325, damping: 30, mass: .72 };
  const closed = animate
    ? { opacity: 0, x: pose.x, y: pose.y, scale: pose.scale, filter: "blur(4px)", borderRadius: "50%" }
    : { opacity: 0, transition: { duration: 0 } };

  return <motion.section
    key={`${kind}-${source?.id ?? "center"}-${pose.ready ? "ready" : "measuring"}`}
    ref={sheetRef}
    className={className}
    style={{ transformOrigin: `${pose.originX} ${pose.originY}`, visibility: pose.ready ? "visible" : "hidden" }}
    role={role}
    tabIndex={-1}
    onKeyDown={handleKeyDown}
    aria-modal="true"
    aria-labelledby={labelledBy}
    initial={animate && pose.ready ? { ...closed, borderRadius: "50%" } : false}
    animate={{ opacity: 1, x: 0, y: 0, scale: 1, filter: "blur(0px)", borderRadius: "25px" }}
    exit={closed}
    transition={animate ? spring : { duration: 0 }}
  >{children}</motion.section>;
}

function useDisableBrowserContextMenu() {
  useEffect(() => {
    const preventBrowserContextMenu = (event: MouseEvent) => {
      event.preventDefault();
      event.stopPropagation();
    };
    document.addEventListener("contextmenu", preventBrowserContextMenu, true);
    return () => document.removeEventListener("contextmenu", preventBrowserContextMenu, true);
  }, []);
}

function App() {
  useDisableBrowserContextMenu();
  useBrandedTooltips();
  useDisplayRefreshRate();
  const windowContext = window as Window & { __CODEX_TRAY_MENU__?: boolean };
  const search = new URLSearchParams(window.location.search);
  if (search.has("installer")) {
    return <InstallerApp />;
  }
  if (windowContext.__CODEX_TRAY_MENU__ || search.has("tray-menu") || window.location.hash === "#tray-menu") {
    return <TrayMenu />;
  }
  return <DashboardApp />;
}

function InstallerApp() {
  const [snapshot, setSnapshot] = useState<InstallerSnapshot | null>(null);
  const [path, setPath] = useState("");
  const [theme, setTheme] = useState<"dark" | "light">(() => window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark");
  const [language, setLanguageState] = useState<Language>(() => getLanguage());
  // Keep the installer preference independent from the dashboard preference:
  // the setup window can be muted before the application has been installed.
  const [soundEnabled, setSoundEnabled] = useState(() => {
    try {
      return window.localStorage.getItem("codex-accounts.installer-sound-enabled") !== "false";
    } catch {
      return true;
    }
  });
  const soundEnabledRef = useRef(soundEnabled);
  const [effectsEnabled, setEffectsEnabled] = useState(() => {
    try {
      return window.localStorage.getItem("codex-accounts.installer-effects-enabled") !== "false";
    } catch {
      return true;
    }
  });
  const effectsEnabledRef = useRef(effectsEnabled);
  const [phase, setPhase] = useState<"welcome" | "installing" | "finish">("welcome");
  const [uninstall, setUninstall] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [closing, setClosing] = useState(false);
  const [languageTransition, setLanguageTransition] = useState(false);
  const languageTransitionTimer = useRef<number | null>(null);
  const installerGlassFrame = useRef<number | null>(null);
  const installerGlassPosition = useRef<{
    root: HTMLDivElement;
    surface: HTMLElement | null;
    clientX: number;
    clientY: number;
  } | null>(null);
  const installerGlassSurface = useRef<HTMLElement | null>(null);
  const installerBackdropGeneration = useRef(0);
  const prefersReducedMotion = usePrefersReducedMotion();

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let cancelled = false;
    const generation = ++installerBackdropGeneration.current;
    const currentWindow = getCurrentWindow();
    const currentWebviewWindow = getCurrentWebviewWindow();
    const base: [number, number, number, number] = theme === "light" ? [255, 255, 255, 255] : [35, 26, 45, 255];
    const setSurfaceBackground = async (color: [number, number, number, number]) => {
      await currentWebviewWindow.setBackgroundColor(color).catch(async () => {
        await currentWindow.setBackgroundColor(color).catch(() => undefined);
      });
    };
    const clearDomGlass = () => {
      document.documentElement.classList.remove("native-glass");
      document.body.classList.remove("native-glass");
    };
    const syncBackdrop = async () => {
      if (!effectsEnabled) {
        await setSurfaceBackground(base);
        if (cancelled || generation !== installerBackdropGeneration.current) return;
        clearDomGlass();
        await currentWindow.clearEffects().catch(() => undefined);
        return;
      }
      await setSurfaceBackground(base);
      if (cancelled || generation !== installerBackdropGeneration.current) return;
      try {
        try {
          await currentWindow.setEffects({
            effects: [Effect.Mica],
          });
        } catch {
          // Mica is Windows 11 only. Windows 10 keeps the same glass design
          // with the tinted native blur fallback.
          await currentWindow.clearEffects().catch(() => undefined);
          await currentWindow.setEffects({
            effects: [Effect.Blur],
            color: theme === "light" ? [247, 244, 251, 224] : [49, 37, 61, 220],
          });
        }
        if (cancelled || generation !== installerBackdropGeneration.current) return;
        document.documentElement.classList.add("native-glass");
        document.body.classList.add("native-glass");
        // WebView2 treats every non-zero background alpha as fully opaque.
        // Keep the opaque theme fill while the effect initializes, then clear
        // the WebView surface so the native Mica/Blur material can show.
        await setSurfaceBackground([0, 0, 0, 0]);
      } catch {
        clearDomGlass();
        await setSurfaceBackground(base);
      }
    };
    void syncBackdrop();
    return () => { cancelled = true; };
  }, [effectsEnabled, theme]);

  useEffect(() => {
    if (effectsEnabled) return;
    if (installerGlassFrame.current !== null) window.cancelAnimationFrame(installerGlassFrame.current);
    installerGlassFrame.current = null;
    installerGlassPosition.current?.root.style.setProperty("--pointer-opacity", "0");
    installerGlassPosition.current = null;
    deactivatePointerGlassSurface(installerGlassSurface.current);
    installerGlassSurface.current = null;
  }, [effectsEnabled]);

  useEffect(() => () => {
    if (installerGlassFrame.current !== null) window.cancelAnimationFrame(installerGlassFrame.current);
    deactivatePointerGlassSurface(installerGlassSurface.current);
  }, []);

  const installerSound = useCallback((kind: string, force = false) => {
    sound(kind, soundEnabledRef.current, force);
  }, []);

  useEffect(() => {
    soundEnabledRef.current = soundEnabled;
    try {
      window.localStorage.setItem("codex-accounts.installer-sound-enabled", String(soundEnabled));
    } catch {
      // The in-memory setting remains effective when storage is unavailable.
    }
  }, [soundEnabled]);

  useEffect(() => {
    effectsEnabledRef.current = effectsEnabled;
    try {
      window.localStorage.setItem("codex-accounts.installer-effects-enabled", String(effectsEnabled));
    } catch {
      // The in-memory preference remains effective when storage is unavailable.
    }
  }, [effectsEnabled]);

  const triggerLanguageTransition = (next: Language) => {
    setLanguage(next);
    setLanguageState(next);
    if (!prefersReducedMotion) {
      setLanguageTransition(true);
      if (languageTransitionTimer.current !== null) window.clearTimeout(languageTransitionTimer.current);
      languageTransitionTimer.current = window.setTimeout(() => setLanguageTransition(false), 460);
    }
  };

  useEffect(() => {
    document.documentElement.lang = language;
  }, [language]);

  useEffect(() => () => {
    if (languageTransitionTimer.current !== null) window.clearTimeout(languageTransitionTimer.current);
  }, []);

  useEffect(() => {
    void invoke<InstallerSnapshot>("installer_snapshot")
      .then((value) => {
        setSnapshot(value);
        setPath(value.selectedPath);
        setUninstall(value.uninstall);
        if (value.finish) setPhase("finish");
        installerSound(value.finish ? (value.uninstall ? "installer-uninstall-finish" : "installer-finish") : "installer-startup");
      })
      .catch((reason) => setError(getErrorText(reason)));
  }, [installerSound]);

  const chooseDirectory = async () => {
    installerSound("installer-browse-open");
    try {
      const selected = await open({ directory: true, multiple: false, title: t("Выберите папку установки Codex Accounts") });
      if (typeof selected === "string" && selected.trim()) {
        setPath(selected);
        installerSound("installer-browse");
      }
    } catch (reason) {
      setError(getErrorText(reason));
      installerSound("installer-browse-error");
    }
  };

  const close = async () => {
    if (closing) return;
    setClosing(true);
    // Closing remains audible when the user is turning sound off elsewhere,
    // but respects the installer's own mute switch.
    installerSound("installer-close");
    if (effectsEnabled && !prefersReducedMotion) {
      await new Promise<void>((resolve) => window.setTimeout(resolve, 220));
    }
    await invoke("installer_close").catch((reason) => {
      setClosing(false);
      setError(getErrorText(reason));
    });
  };

  const prepare = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    setPhase("installing");
    installerSound(uninstall ? "installer-uninstall" : "installer-prepare");
    try {
      await invoke(uninstall ? "installer_uninstall" : "installer_prepare", uninstall ? {} : { path });
      setPhase("finish");
      setBusy(false);
      installerSound(uninstall ? "installer-uninstall-finish" : "installer-finish");
    } catch (reason) {
      setPhase("welcome");
      setBusy(false);
      setError(getErrorText(reason));
      installerSound("installer-prepare-error");
    }
  };

  const launch = async () => {
    if (busy) return;
    setBusy(true);
    installerSound("installer-launch");
    try {
      await invoke("installer_launch", { path });
    } catch (reason) {
      setBusy(false);
      setError(getErrorText(reason));
      installerSound("installer-launch-error");
    }
  };

  const openInstallFolder = async () => {
    if (busy) return;
    installerSound("installer-open-folder");
    try {
      await invoke("installer_open_folder", { path });
    } catch (reason) {
      setError(getErrorText(reason));
      installerSound("installer-open-folder-error");
    }
  };

  const minimizeInstaller = async () => {
    installerSound("installer-minimize");
    await getCurrentWindow().minimize().catch((reason) => setError(getErrorText(reason)));
  };

  const toggleTheme = () => {
    setTheme((current) => current === "dark" ? "light" : "dark");
    installerSound("installer-theme");
  };

  const toggleLanguage = () => {
    const next: Language = language === "ru" ? "en" : "ru";
    triggerLanguageTransition(next);
    installerSound(next === "en" ? "installer-language-en" : "installer-language-ru");
  };

  const toggleSound = () => {
    const next = !soundEnabledRef.current;
    soundEnabledRef.current = next;
    setSoundEnabled(next);
    // Play the acknowledgement even for the transition into the muted state;
    // after this event all later installer actions stay silent.
    sound(next ? "installer-sound-on" : "installer-sound-off", true);
  };

  const toggleEffects = () => {
    const next = !effectsEnabledRef.current;
    effectsEnabledRef.current = next;
    setEffectsEnabled(next);
    installerSound(next ? "installer-effects-on" : "installer-effects-off");
  };

  const trackInstallerGlass = (event: React.PointerEvent<HTMLDivElement>) => {
    const root = event.currentTarget;
    if (!effectsEnabled || event.pointerType !== "mouse") {
      if (installerGlassFrame.current !== null) window.cancelAnimationFrame(installerGlassFrame.current);
      installerGlassFrame.current = null;
      installerGlassPosition.current = null;
      deactivatePointerGlassSurface(installerGlassSurface.current);
      installerGlassSurface.current = null;
      root.style.setProperty("--pointer-opacity", "0");
      return;
    }
    const target = event.target instanceof Element ? event.target : null;
    const surface = target?.closest<HTMLElement>("button, input, .installer-feature-list div") ?? (
      target?.closest(".installer-path-label")
        ? null
        : target?.closest<HTMLElement>(".installer-card") ?? null
    );
    installerGlassPosition.current = { root, surface, clientX: event.clientX, clientY: event.clientY };
    if (installerGlassFrame.current !== null) return;
    installerGlassFrame.current = window.requestAnimationFrame(() => {
      installerGlassFrame.current = null;
      const position = installerGlassPosition.current;
      if (!position || !position.root.isConnected) return;
      const rootRect = position.root.getBoundingClientRect();
      position.root.style.setProperty("--pointer-x", `${position.clientX - rootRect.left}px`);
      position.root.style.setProperty("--pointer-y", `${position.clientY - rootRect.top}px`);
      position.root.style.setProperty("--pointer-opacity", "1");
      if (installerGlassSurface.current !== position.surface) {
        deactivatePointerGlassSurface(installerGlassSurface.current);
        installerGlassSurface.current = position.surface;
        activatePointerGlassSurface(installerGlassSurface.current);
      }
      const glassSurface = installerGlassSurface.current;
      if (!glassSurface?.isConnected) return;
      const rect = glassSurface.getBoundingClientRect();
      const x = Math.max(0, Math.min(rect.width, position.clientX - rect.left));
      const y = Math.max(0, Math.min(rect.height, position.clientY - rect.top));
      const normalizedX = rect.width > 0 ? x / rect.width - 0.5 : 0;
      const normalizedY = rect.height > 0 ? y / rect.height - 0.5 : 0;
      glassSurface.style.setProperty("--glass-x", `${x}px`);
      glassSurface.style.setProperty("--glass-y", `${y}px`);
      glassSurface.style.setProperty("--glass-tilt-x", `${normalizedX * 1.4}deg`);
      glassSurface.style.setProperty("--glass-tilt-y", `${normalizedY * -1.2}deg`);
      glassSurface.style.setProperty("--glass-stretch-x", String(1.018 + normalizedX * 0.028));
      glassSurface.style.setProperty("--glass-stretch-y", String(1.014 - normalizedY * 0.022));
    });
  };

  const clearInstallerGlass = (root: HTMLDivElement) => {
    if (installerGlassFrame.current !== null) window.cancelAnimationFrame(installerGlassFrame.current);
    installerGlassFrame.current = null;
    installerGlassPosition.current = null;
    deactivatePointerGlassSurface(installerGlassSurface.current);
    installerGlassSurface.current = null;
    root.style.setProperty("--pointer-opacity", "0");
  };

  const animateInstaller = effectsEnabled && !prefersReducedMotion;
  const enterInstallerState = (y: number, scale = 1) => animateInstaller ? { opacity: 0, y, scale } : false;
  const exitInstallerState = animateInstaller ? { opacity: 0, y: -6 } : { opacity: 0, transition: { duration: 0 } };
  const isFinish = phase === "finish";
  const isInstalling = phase === "installing";
  return (
    <MotionConfig reducedMotion={prefersReducedMotion ? "always" : "never"}>
    <motion.div className={`installer-shell theme-${theme} ${soundEnabled ? "sound-on" : "sound-off"} ${effectsEnabled ? "effects-on" : "effects-off"} installer-${phase} ${uninstall ? "installer-uninstall" : ""} ${closing ? "installer-closing" : ""} ${languageTransition ? "language-transition" : ""}`} initial={false} animate={{ opacity: 1, scale: 1 }} transition={{ duration: effectsEnabled && !prefersReducedMotion ? 0.36 : 0, ease: [0.2, 0.8, 0.2, 1] }} onPointerDownCapture={beginButtonAnimationEvent} onPointerMove={trackInstallerGlass} onPointerLeave={(event) => clearInstallerGlass(event.currentTarget)} onClickCapture={beginButtonSoundEvent} onClick={(event) => finishButtonSoundEvent(event, soundEnabledRef.current)}>
        <div className="installer-atmosphere installer-atmosphere-one" />
        <div className="installer-atmosphere installer-atmosphere-two" />
        <header className="installer-titlebar" data-tauri-drag-region>
          <div className="brand" data-tauri-drag-region>
            <BrandMark installer />
            <div className="brand-copy"><b>Codex Accounts</b><small>{t("УСТАНОВКА · ПРОФИЛИ · ЛИМИТЫ")}</small></div>
          </div>
          <div className="installer-toolbar">
            <button type="button" className="installer-tool" data-feedback="installer-theme" onClick={toggleTheme} aria-label={t(theme === "dark" ? "Переключить на светлую тему" : "Переключить на тёмную тему")} title={t(theme === "dark" ? "Переключить на светлую тему" : "Переключить на тёмную тему")}>{theme === "dark" ? <Moon size={15} /> : <Sun size={15} />}</button>
            <button type="button" className="installer-tool installer-language-text" data-feedback="installer-language" onClick={toggleLanguage} aria-label={languageSwitchHint(language === "ru" ? "en" : "ru")} title={languageSwitchHint(language === "ru" ? "en" : "ru")}><span>{language.toUpperCase()}</span></button>
            <ToolbarButton feedback="installer-sound" className="installer-sound-toggle" label={soundEnabled ? "Выключить звук" : "Включить звук"} onClick={toggleSound} icon={soundEnabled ? <Volume2 className="installer-sound-icon" size={17} strokeWidth={1.8} /> : <VolumeX className="installer-sound-icon" size={17} strokeWidth={1.8} />} active={soundEnabled} />
            <ToolbarButton feedback="installer-effects" className="installer-effects-toggle" label={effectsEnabled ? "Выключить визуальные эффекты" : "Включить визуальные эффекты"} onClick={toggleEffects} icon={<Waves className="installer-effects-icon" size={17} strokeWidth={1.8} />} active={effectsEnabled} />
            <button type="button" className="installer-window-minimize" data-feedback="installer-minimize" onClick={() => void minimizeInstaller()} aria-label={t("Свернуть установщик")} title={t("Свернуть установщик")} disabled={busy || closing}><Minus size={15} /></button>
            <button type="button" className="installer-window-close" data-feedback="installer-close" onClick={() => void close()} aria-label={t("Закрыть установщик")} title={t("Закрыть установщик")} disabled={busy || closing}><X size={15} /></button>
          </div>
        </header>

        <main className="installer-main">
          <section className="installer-hero">
            <p className="eyebrow"><span className="eyebrow-line" />{uninstall ? t("УДАЛЕНИЕ · СОХРАНЕНИЕ ДАННЫХ") : t("ЛОКАЛЬНАЯ УСТАНОВКА · WINDOWS 10/11")}</p>
            <AnimatePresence mode="wait">
              {isInstalling ? (
                <motion.div key="installing" className="installer-state" initial={enterInstallerState(8)} animate={{ opacity: 1, y: 0 }} exit={exitInstallerState}>
                  <div className="installer-spinner"><span /><span /><span /></div>
                  <h1>{t(uninstall ? "Готовим удаление" : "Готовим установку")}</h1>
                  <p>{t(uninstall ? "Готовим приложение к безопасному удалению." : "Передаём выбранную папку установщику и бережно сохраняем ваши данные.")}</p>
                </motion.div>
              ) : isFinish ? (
                <motion.div key="finish" className="installer-state" initial={enterInstallerState(3, .97)} animate={{ opacity: 1, y: 0, scale: 1 }} exit={exitInstallerState}>
                  <div className="installer-success-mark"><Check size={28} /></div>
                  <h1>{t(uninstall ? "Готово к завершению удаления" : "Всё готово")}</h1>
                  <p>{t(uninstall ? "Файлы приложения будут удалены после закрытия окна. Профили сохранены." : "Codex Accounts установлен. Откройте приложение, чтобы управлять профилями и лимитами.")}</p>
                </motion.div>
              ) : (
                <motion.div key="welcome" initial={enterInstallerState(5)} animate={{ opacity: 1, y: 0 }} exit={exitInstallerState}>
                  {uninstall ? (
                    <>
                      <h1>{t("Удалить Codex Accounts?")}</h1>
                      <p className="installer-lead">{t("Файлы приложения и ярлыки будут удалены. Профили, лимиты и настройки пользователя останутся на месте.")}</p>
                    </>
                  ) : (
                    <>
                      <h1>{t("Codex — под контролем")}</h1>
                      <p className="installer-lead">{t("Профили, лимиты и общие настройки в одном спокойном рабочем пространстве.")}</p>
                      <div className="installer-feature-list">
                        <div><b aria-hidden="true"><Route size={16} strokeWidth={1.8} /></b><span>{t("Профили Codex в одном месте")}</span></div>
                        <div><b aria-hidden="true"><HardDriveDownload size={16} strokeWidth={1.8} /></b><span>{t("Общие настройки и резервные копии")}</span></div>
                      </div>
                    </>
                  )}
                </motion.div>
              )}
            </AnimatePresence>
          </section>

          <motion.section className="installer-card" layout={animateInstaller} transition={{ type: "spring", stiffness: 360, damping: 32 }}>
            <div className="installer-card-orbit" aria-hidden="true"><div className="installer-orbit-ring" /><div className="installer-orbit-core"><ProductGlyph /></div><i /><i /><i /></div>
            {isFinish ? (
              <div className="installer-card-content"><span className="installer-card-kicker">{t(uninstall ? "ПОДГОТОВКА ЗАВЕРШЕНА" : "УСТАНОВКА ЗАВЕРШЕНА")}</span><h2>{t(uninstall ? "Данные сохранены" : "Можно начинать работу")}</h2><p>{t(uninstall ? "Закройте окно, чтобы удалить файлы приложения и ярлыки. Данные пользователя сохранятся." : "Ярлыки и файлы приложения уже на месте. Профили и настройки пользователя не менялись.")}</p><div className="installer-card-actions"><button type="button" className="button-secondary" data-feedback="installer-dismiss" onClick={() => void close()} disabled={busy} title={t(uninstall ? "Завершить удаление" : "Закрыть")}>{t(uninstall ? "Завершить удаление" : "Закрыть")}</button>{!uninstall && <><button type="button" className="button-secondary" data-feedback="installer-folder" onClick={() => void openInstallFolder()} disabled={busy} title={t("Открыть папку")}><FolderOpen size={16} />{t("Открыть папку")}</button><button type="button" className="button-primary" data-feedback="installer-launch" onClick={() => void launch()} disabled={busy} title={t("Открыть Codex Accounts")}><ArrowRight size={16} />{t("Открыть Codex Accounts")}</button></>}</div></div>
            ) : isInstalling ? (
              <div className="installer-card-content"><span className="installer-card-kicker">{t("СЛЕДУЮЩИЙ ШАГ")}</span><h2>{t(uninstall ? "Сохраняем данные" : "Проверяем файлы")}</h2><p>{t(uninstall ? "Подготавливаем настройки и локальный маршрут Codex." : "Копируем файлы приложения и создаём ярлыки.")}</p><div className="installer-progress"><span /></div><small>{path}</small></div>
            ) : (
              <div className="installer-card-content"><span className="installer-card-kicker">{t(uninstall ? "ПАПКА ПРИЛОЖЕНИЯ" : "ПАПКА УСТАНОВКИ")}</span><h2>{t(uninstall ? "Удалить программу" : "Готово к установке")}</h2><p>{t(uninstall ? "Будут удалены только файлы приложения и ярлыки. Профили и настройки пользователя останутся в Windows." : "WebView2 устанавливается только при необходимости. Профили остаются в вашем профиле Windows.")}</p><label className="installer-path-label">{t("Папка приложения")}<div className="installer-path"><input value={path} onChange={(event) => setPath(event.target.value)} readOnly={uninstall} spellCheck={false} aria-label={t("Папка установки")} />{!uninstall && <button type="button" data-feedback="installer-browse" onClick={() => void chooseDirectory()} aria-label={t("Выбрать папку")} title={t("Выбрать папку")}><FolderOpen size={16} /></button>}</div></label><div className="installer-card-actions"><button type="button" className="button-secondary" data-feedback="installer-dismiss" onClick={() => void close()} disabled={busy} title={t("Отмена")}>{t("Отмена")}</button><button type="button" className="button-primary" data-feedback={uninstall ? "installer-uninstall" : "installer-install"} onClick={() => void prepare()} disabled={busy || (!uninstall && !path.trim())} title={t(uninstall ? "Удалить программу" : "Установить")}>{uninstall ? <Trash2 size={16} /> : <Check size={17} strokeWidth={2.8} />}{t(uninstall ? "Удалить программу" : "Установить")}</button></div></div>
            )}
            {error && <div className="installer-error"><XCircle size={15} /><span>{error}</span></div>}
          </motion.section>
        </main>
        <footer className="installer-footer"><small>v{snapshot?.appVersion ?? "0.1.0"}</small></footer>
      </motion.div>
    </MotionConfig>
  );
}

function DashboardApp() {
  const [data, setData] = useState<DashboardData | null>(null);
  const [language, setLanguageState] = useState<Language>(() => getLanguage());
  const [languageTransition, setLanguageTransition] = useState(false);
  const languageTransitionTimer = useRef<number | null>(null);
  const prefersReducedMotion = usePrefersReducedMotion();
  const visualEffectsPreference = useRef(true);
  const [modal, setModal] = useState<Modal>(null);
  const [modalOrigin, setModalOrigin] = useState<ModalOrigin>("center");
  const [modalSource, setModalSource] = useState<MotionSource>(null);
  const [authSource, setAuthSource] = useState<MotionSource>(null);
  const [pendingRemove, setPendingRemove] = useState<Account | null>(null);
  const [busy, setBusy] = useState("");
  const [toast, setToast] = useState<Toast | null>(null);
  const [limitAlert, setLimitAlert] = useState<LimitAlertData | null>(null);
  const [accountSearch, setAccountSearch] = useState("");
  const accountSearchInput = useRef<HTMLInputElement | null>(null);
  const accountSearchPlaceholder = t("Имя, почта или тариф");
  const accountListRef = useRef<HTMLDivElement | null>(null);
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [settingsDraft, setSettingsDraft] = useState<AppSettings | null>(null);
  const [preferencesDraft, setPreferencesDraft] = useState<CodexPreferences>(initialPreferences);
  const [selectedPrompt, setSelectedPrompt] = useState<Prompt | null>(null);
  const [soundEnabled, setSoundEnabled] = useState(true);
  const soundEnabledRef = useRef(true);
  const [addingAccount, setAddingAccount] = useState(false);
  const [manualAuthLink, setManualAuthLink] = useState(false);
  const [addStatus, setAddStatus] = useState("");
  const [authUrl, setAuthUrl] = useState("");
  const [authUserCode, setAuthUserCode] = useState("");
  // A copy request can arrive before the CLI has printed its OAuth URL.  Keep
  // that intent alive until the manual, no-browser login flow emits the URL;
  // the event handler then copies it immediately without opening a browser.
  const pendingAuthCopy = useRef(false);
  const [windowVisible, setWindowVisible] = useState(true);
  const [usageClock, setUsageClock] = useState(() => Math.floor(Date.now() / 1000));
  const [systemTheme, setSystemTheme] = useState<"dark" | "light">(() =>
    window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark",
  );
  const [animatingWindow, setAnimatingWindow] = useState("");
  const [effectsTransition, setEffectsTransition] = useState("");
  const [accountColumns, setAccountColumns] = useState<AccountColumns>(() => {
    return readAccountView("columns", 2);
  });
  const [accountSort, setAccountSort] = useState<AccountSort>(() => {
    return readAccountView("sort", "remaining");
  });
  const [accountSortReversed, setAccountSortReversed] = useState(() => readAccountView("sortReversed", false));
  const busyRef = useRef("");
  const backupDialogOpen = useRef(false);
  const disposedRef = useRef(false);
  const toastId = useRef(0);
  const activityEventSequence = useRef(0);
  const toastTimers = useRef<Set<number>>(new Set());
  const previousActive = useRef<string | null>(null);
  const dashboardRevision = useRef(0);
  const dashboardRead = useRef(0);
  const appliedDashboardRead = useRef(0);
  const lastTrayTheme = useRef<AppSettings["theme"] | null>(null);
  const previousAccountStatuses = useRef<Map<string, AccountStatusKind> | null>(null);
  const previousThresholdWarnings = useRef<Map<string, boolean> | null>(null);
  const manualSwitch = useRef(false);
  const updateTimer = useRef<number | null>(null);
  const windowAnimationTimer = useRef<number | null>(null);
  const effectsTransitionTimer = useRef<number | null>(null);
  const lifecycleTimer = useRef<number | null>(null);
  const lifecycleClearTimer = useRef<number | null>(null);
  const nativeGlassWarningShown = useRef(false);
  const backdropSyncGeneration = useRef(0);
  const resizeSoundTimer = useRef<number | null>(null);
  const nativeResizeGuardTimer = useRef<number | null>(null);
  const nativeResizeCommand = useRef(false);
  const resizeInProgress = useRef(false);
  const pointerFrame = useRef<number | null>(null);
  const pointerPosition = useRef<{
    target: HTMLElement;
    x: number;
    y: number;
    surface: HTMLElement | null;
    clientX: number;
    clientY: number;
  } | null>(null);
  const pointerGlassSurface = useRef<HTMLElement | null>(null);

  const setBusyState = useCallback((value: string) => {
    busyRef.current = value;
    setBusy(value);
  }, []);

  const recordActivity = useCallback((title: string, detail: string, tone = "info") => {
    if (disposedRef.current) return;
    const id = Date.now() * 1000 + activityEventSequence.current++;
    setEvents((items) => [{ id, at: Math.floor(Date.now() / 1000), title, detail, tone }, ...items].slice(0, 8));
  }, []);

  const triggerLanguageTransition = useCallback((next: Language) => {
    setLanguage(next);
    setLanguageState(next);
    recordActivity(
      "Язык интерфейса изменён",
      next === "ru" ? "Включён русский язык интерфейса." : "Включён английский язык интерфейса.",
    );
    if (prefersReducedMotion) return;
    setLanguageTransition(true);
    if (languageTransitionTimer.current !== null) window.clearTimeout(languageTransitionTimer.current);
    languageTransitionTimer.current = window.setTimeout(() => setLanguageTransition(false), 460);
  }, [prefersReducedMotion, recordActivity]);

  const sourceRectForEvent = useCallback((event?: React.MouseEvent<HTMLElement>): MotionSource => {
    const target = event?.currentTarget;
    if (!(target instanceof HTMLElement)) return null;
    const rect = target.getBoundingClientRect();
    return { id: Date.now() + Math.round(rect.left + rect.top), left: rect.left, top: rect.top, width: rect.width, height: rect.height };
  }, []);

  const rememberModalSource = useCallback((origin: ModalOrigin, event?: React.MouseEvent<HTMLElement>) => {
    setModalOrigin(origin);
    setModalSource(sourceRectForEvent(event));
  }, [sourceRectForEvent]);
  visualEffectsPreference.current = data?.settings.visualEffectsEnabled ?? true;

  useEffect(() => {
    document.documentElement.lang = language;
  }, [language]);

  useEffect(() => {
    disposedRef.current = false;
    return () => {
      disposedRef.current = true;
      if (lifecycleTimer.current !== null) window.clearTimeout(lifecycleTimer.current);
      if (lifecycleClearTimer.current !== null) window.clearTimeout(lifecycleClearTimer.current);
      lifecycleTimer.current = null;
      lifecycleClearTimer.current = null;
      if (updateTimer.current !== null) window.clearTimeout(updateTimer.current);
      if (windowAnimationTimer.current !== null) window.clearTimeout(windowAnimationTimer.current);
    if (languageTransitionTimer.current !== null) window.clearTimeout(languageTransitionTimer.current);
    if (effectsTransitionTimer.current !== null) window.clearTimeout(effectsTransitionTimer.current);
    if (pointerFrame.current !== null) window.cancelAnimationFrame(pointerFrame.current);
    toastTimers.current.forEach((timer) => window.clearTimeout(timer));
    toastTimers.current.clear();
    };
  }, []);

  useEffect(() => {
    if (!windowVisible) return;
    setUsageClock(Math.floor(Date.now() / 1000));
    const timer = window.setInterval(() => setUsageClock(Math.floor(Date.now() / 1000)), 30_000);
    return () => window.clearInterval(timer);
  }, [windowVisible]);

  const clearResizeState = useCallback(() => {
    document.documentElement.classList.remove("native-resizing", "native-resizing-soft");
    document.body.classList.remove("native-resizing", "native-resizing-soft");
    document.documentElement.style.removeProperty("--resize-backdrop");
    resizeInProgress.current = false;
    nativeResizeCommand.current = false;
    setAnimatingWindow((current) => current === "window-resize-live" ? "" : current);
  }, []);

  const showToast = useCallback((title: string, detail?: string, tone: Toast["tone"] = "ok") => {
    if (disposedRef.current) return;
    const id = ++toastId.current;
    setToast({ id, title, detail, tone });
    const timer = window.setTimeout(() => {
      toastTimers.current.delete(timer);
      if (disposedRef.current) return;
      setToast((current) => current?.id === id ? null : current);
    }, 4400);
    toastTimers.current.add(timer);
    if (tone === "error" || tone === "warning") sound(`${tone}:${title}:${detail ?? ""}`, soundEnabledRef.current);
    else if (tone === "info") sound(`notice:${title}:${detail ?? ""}`, soundEnabledRef.current);
    recordActivity(title, detail ?? (tone === "error" ? "Проверьте сообщение и попробуйте снова." : "Готово."), tone);
  }, [recordActivity]);

  const applyDashboard = useCallback((next: DashboardData, fromRead = false) => {
    if (disposedRef.current) return;
    if (!fromRead) dashboardRevision.current += 1;
    const previousActiveId = previousActive.current;
    const newlyLimitedPreviousActive = next.settings.automaticProfileSwitching && !manualSwitch.current
      && !!previousActiveId
      && previousActiveId !== next.currentAccountId
      && next.accounts.some((account) => account.id === previousActiveId && account.status?.kind === "usageLimit")
      ? previousActiveId
      : null;
    if (lastTrayTheme.current !== next.settings.theme) {
      lastTrayTheme.current = next.settings.theme;
      void emitTo("tray-menu", "tray-theme-update", next.settings.theme).catch(() => undefined);
    }
    setLimitAlert((current) => {
      if (!current) return current;
      const account = next.accounts.find((candidate) => candidate.id === current.accountId);
      if (!account || !account.isActive || displayAccountStatus(account, Math.floor(Date.now() / 1000)).kind !== "usageLimit") return null;
      const status = accountStatusInfo(account.status);
      return { accountId: account.id, email: account.email, detail: status.detail || status.hint, resetAt: nextResetAt(account) };
    });
    if (previousActive.current && next.currentAccountId && previousActive.current !== next.currentAccountId) {
      sound(manualSwitch.current ? "switch" : "autoSwitch", next.settings.soundEnabled);
    }
    if (newlyLimitedPreviousActive) {
      const exhausted = next.accounts.find((account) => account.id === newlyLimitedPreviousActive);
      const nextAccount = next.accounts.find((account) => account.id === next.currentAccountId);
      if (exhausted && nextAccount) {
        showToast(
          "Лимит исчерпан — профиль переключён",
          `${exhausted.email}: выбран следующий доступный профиль ${nextAccount.email}. Остановленный запрос не повторён; новые чаты используют выбранный профиль.`,
          "info",
        );
        setLimitAlert(null);
      }
    }
    setSoundEnabled(next.settings.soundEnabled);
    soundEnabledRef.current = next.settings.soundEnabled;
    previousActive.current = next.currentAccountId;
    if (!next.adminUnlocked && next.adminProtected) {
      previousAccountStatuses.current = null;
      previousThresholdWarnings.current = null;
    } else {
      const previousStatuses = previousAccountStatuses.current;
      const previousWarnings = previousThresholdWarnings.current;
      const nextStatuses = new Map<string, AccountStatusKind>();
      const nextWarnings = new Map<string, boolean>();
      for (const account of next.accounts) {
        const status = accountStatusInfo(account.status);
        nextStatuses.set(account.id, status.kind);
        const oldKind = previousStatuses?.get(account.id);
        const visibleKind = displayAccountStatus(account, Math.floor(Date.now() / 1000)).kind;
        const remainingForWarning = account.id === next.currentAccountId ? availableNow(account) : null;
        const warningReached = account.isActive && visibleKind === "healthy" && remainingForWarning !== null
          && remainingForWarning > 0 && remainingForWarning <= next.settings.switchThreshold;
        const wasWarningReached = previousWarnings?.get(account.id) ?? false;
        nextWarnings.set(account.id, warningReached);
        if (warningReached && !wasWarningReached) {
          showToast("Порог остатка достигнут", `${account.email}: осталось ${remainingForWarning}%. Можно выбрать другой профиль вручную.`, "warning");
        }
        const limitSwitchGuidance = next.settings.automaticProfileSwitching
          ? "Автоматический переход не выполнен; выберите доступный профиль вручную."
          : "Автоматическое переключение отключено. Выберите доступный профиль вручную.";
        if (previousStatuses === null && (!next.adminProtected || next.adminUnlocked) && visibleKind === "usageLimit" && account.isActive) {
          const detail = status.detail || status.hint;
          showToast("Лимит аккаунта исчерпан", `${account.email}: ${detail} ${limitSwitchGuidance} Остановленный запрос не будет повторён.`, "warning");
          setLimitAlert({ accountId: account.id, email: account.email, detail, resetAt: nextResetAt(account) });
        }
        if (status.permanent && oldKind !== status.kind && (oldKind !== undefined || previousStatuses !== null)) {
          const reason = status.detail || status.hint;
          const guidance = reason === status.hint ? "" : ` ${status.hint}`;
          showToast(
            `Профиль ${account.email} недоступен`,
            `${status.label}. ${reason}${guidance} Исправьте вход или выберите другой профиль вручную.`,
            "error",
          );
        } else if (status.kind === "usageLimit" && oldKind !== "usageLimit" && previousStatuses !== null && account.isActive) {
          const detail = status.detail || status.hint;
          showToast("Лимит аккаунта исчерпан", `${account.email}: ${detail} ${limitSwitchGuidance} Остановленный запрос не будет повторён.`, "warning");
          setAccountSearch("");
          setLimitAlert({ accountId: account.id, email: account.email, detail, resetAt: nextResetAt(account) });
        } else if (status.kind === "usageLimit" && oldKind !== "usageLimit" && previousStatuses !== null && account.id !== newlyLimitedPreviousActive) {
          showToast("Лимит профиля исчерпан", `${account.email}: лимит подтверждён в уже открытом чате. Активный профиль не изменён; запрос не повторён.`, "warning");
        } else if (status.kind === "temporaryError" && oldKind !== "temporaryError" && previousStatuses !== null) {
          showToast("Не удалось проверить аккаунт", `${account.email}: ${status.detail || status.hint}`, "warning");
        } else if (oldKind && permanentAccountStatuses.includes(oldKind) && status.kind === "healthy") {
          sound(`account-recovered:${account.id}`, next.settings.soundEnabled);
          showToast("Вход восстановлен", `${account.email}: профиль снова доступен.`, "ok");
        } else if (oldKind && oldKind !== "healthy" && status.kind === "healthy") {
          sound(`account-recovered:${account.id}`, next.settings.soundEnabled);
          showToast("Профиль снова доступен", `${account.email}: проверка прошла успешно.`, "ok");
        }
      }
      previousAccountStatuses.current = nextStatuses;
      previousThresholdWarnings.current = nextWarnings;
    }
    setData(next);
  }, [showToast]);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const update = () => setSystemTheme(media.matches ? "light" : "dark");
    update();
    media.addEventListener?.("change", update);
    return () => media.removeEventListener?.("change", update);
  }, []);

  useEffect(() => {
    try {
      window.localStorage.setItem(accountViewStorageKey, JSON.stringify({ columns: accountColumns, sort: accountSort, sortReversed: accountSortReversed }));
    } catch { /* Local storage can be disabled in a locked-down profile. */ }
  }, [accountColumns, accountSort, accountSortReversed]);

  useEffect(() => {
    // The listener is native-only.  Design previews and ordinary browser
    // reloads should keep working without a Tauri bridge.
    if (!("__TAURI_INTERNALS__" in window)) return;
    let dispose: (() => void) | undefined;
    void getCurrentWindow().onResized(() => {
      // Freeze layout and glass work during native resizing, without hiding or
      // replacing the WebView surface. Hiding it can expose a stale snapshot.
      if (!resizeInProgress.current) {
        resizeInProgress.current = true;
        setAnimatingWindow("window-resize-live");
      }
      document.documentElement.classList.add("native-resizing-soft");
      document.body.classList.add("native-resizing-soft");
      if (resizeSoundTimer.current !== null) window.clearTimeout(resizeSoundTimer.current);
      if (nativeResizeGuardTimer.current !== null) window.clearTimeout(nativeResizeGuardTimer.current);
      resizeSoundTimer.current = window.setTimeout(() => {
        resizeSoundTimer.current = null;
        if (!nativeResizeCommand.current) sound("resize", soundEnabledRef.current);
      }, 360);
      nativeResizeGuardTimer.current = window.setTimeout(() => {
        nativeResizeGuardTimer.current = null;
        clearResizeState();
      }, nativeResizeCommand.current ? 420 : 280);
    }).then((unlisten) => { dispose = unlisten; }).catch(() => undefined);
    return () => {
      dispose?.();
      if (resizeSoundTimer.current !== null) window.clearTimeout(resizeSoundTimer.current);
      if (nativeResizeGuardTimer.current !== null) window.clearTimeout(nativeResizeGuardTimer.current);
      document.documentElement.classList.remove("native-resizing", "native-resizing-soft");
      document.body.classList.remove("native-resizing", "native-resizing-soft");
    };
  }, [clearResizeState]);

  useEffect(() => {
    // Tauri's native backdrop keeps the WebView surface transparent only while
    // the window is visible and effects are enabled. Mica is used on Windows
    // 11; Blur is the Windows 10 fallback and avoids Acrylic's resize cost.
    if (!("__TAURI_INTERNALS__" in window)) return;
    let cancelled = false;
    const generation = ++backdropSyncGeneration.current;
    const currentWindow = getCurrentWindow();
    const currentWebviewWindow = getCurrentWebviewWindow();
    const resolved = data?.settings.theme === "dark"
      ? "dark"
      : data?.settings.theme === "light"
        ? "light"
        : systemTheme;
    const base: [number, number, number, number] = resolved === "light" ? [255, 255, 255, 255] : [35, 26, 45, 255];
    const glassEnabled = !!data?.settings.visualEffectsEnabled && windowVisible;
    const clearDomGlass = () => {
      document.documentElement.classList.remove("native-glass");
      document.body.classList.remove("native-glass");
    };
    const syncBackdrop = async () => {
      const setSurfaceBackground = async (color: [number, number, number, number]) => {
        // A transparent Tauri window has two surfaces on Windows: the native
        // frame and the WebView.  Painting only the frame leaves the WebView
        // transparent after a restart/effect toggle, so the desktop behind it
        // (or a black compositor surface) bleeds through.  Set both layers.
        await currentWebviewWindow.setBackgroundColor(color).catch(async () => {
          await currentWindow.setBackgroundColor(color).catch(() => undefined);
        });
      };
      if (!glassEnabled) {
        // Paint an opaque themed surface before clearing the native effect.
        // Clearing the effect is asynchronous; doing it first briefly leaves
        // the transparent WebView composited over whatever window is behind it.
        await setSurfaceBackground(base);
        if (cancelled || generation !== backdropSyncGeneration.current) return;
        clearDomGlass();
        await currentWindow.clearEffects().catch(() => undefined);
        return;
      }
      // Keep the opaque themed surface while the native effect is being
      // installed. Only make the WebView transparent after Mica/Blur is ready;
      // this avoids a one-frame flash of the desktop when effects are enabled.
      await setSurfaceBackground(base);
      if (cancelled || generation !== backdropSyncGeneration.current) return;
      try {
        try {
          await currentWindow.setEffects({
            effects: [Effect.Mica],
          });
        } catch {
          // Mica is Windows 11 only. Windows 10 keeps the same glass design
          // with the tinted native blur fallback.
          await currentWindow.clearEffects().catch(() => undefined);
          await currentWindow.setEffects({
            effects: [Effect.Blur],
            color: resolved === "light" ? [247, 244, 251, 224] : [49, 37, 61, 220],
          });
        }
        if (cancelled || generation !== backdropSyncGeneration.current) return;
        document.documentElement.classList.add("native-glass");
        document.body.classList.add("native-glass");
        // WebView2 treats every non-zero background alpha as fully opaque.
        // Keep the opaque theme fill while the effect initializes, then clear
        // the WebView surface so the native Mica/Blur material can show.
        await setSurfaceBackground([0, 0, 0, 0]);
      } catch {
        // CSS glass remains useful on Windows 10 or a WebView without Mica.
        // Keep the transparent surface and let the themed panels provide the
        // fallback material instead of turning the whole effect off.
        clearDomGlass();
        await setSurfaceBackground(base);
        if (!cancelled && !nativeGlassWarningShown.current) {
          nativeGlassWarningShown.current = true;
          showToast("Стеклянный фон недоступен", "Windows не поддержал системное размытие. Приложение продолжит работу с обычным фоном; включите эффект позже или проверьте версию Windows.", "info");
        }
      }
    };
    void syncBackdrop();
    return () => { cancelled = true; };
  }, [data?.settings.theme, data?.settings.visualEffectsEnabled, systemTheme, windowVisible, showToast]);

  const load = useCallback(async (quiet = false) => {
    const revision = dashboardRevision.current;
    const read = ++dashboardRead.current;
    const isCurrent = () => !disposedRef.current && revision === dashboardRevision.current && read > appliedDashboardRead.current;
    try {
      const next = await invoke<DashboardData>("load_dashboard");
      if (!isCurrent()) return;
      appliedDashboardRead.current = read;
      applyDashboard(next, true);
      if (next.proxyError && !quiet) showToast("Локальный прокси недоступен", next.proxyError, "error");
    } catch (error) {
      if (!quiet && isCurrent()) showToast("Не удалось открыть данные", `${getErrorText(error)} Проверьте доступ к папке приложения.`, "error");
    }
  }, [applyDashboard, showToast]);

  useEffect(() => {
    if (window.location.search.includes("design-preview")) {
      const now = Math.floor(Date.now() / 1000);
      const previewParams = new URLSearchParams(window.location.search);
      const previewTheme = previewParams.get("theme") === "light" ? "light" : "dark";
      const previewConnected = !previewParams.has("disconnected");
      applyDashboard({
        accounts: [
          { id: "preview-alex", accountId: "preview-a", email: "alex.morozov@studio.dev", plan: "ChatGPT Plus", createdAt: now - 86400 * 40, lastUsedAt: now - 1800, isActive: true, status: { kind: "healthy", detail: null }, usage: { primary: { usedPercent: 21, windowMinutes: 300, resetsAt: now + 3600 * 2 }, secondary: { usedPercent: 38, windowMinutes: 10080, resetsAt: now + 86400 * 3 }, updatedAt: now - 48, error: null } },
          { id: "preview-anya", accountId: "preview-b", email: "anya.sokolova@studio.dev", plan: "ChatGPT Team", createdAt: now - 86400 * 18, lastUsedAt: now - 86400, isActive: false, status: { kind: "healthy", detail: null }, usage: { primary: { usedPercent: 42, windowMinutes: 300, resetsAt: now + 3600 * 4 }, secondary: { usedPercent: 55, windowMinutes: 10080, resetsAt: now + 86400 * 2 }, updatedAt: now - 62, error: null } },
          { id: "preview-robot", accountId: "preview-c", email: "codex@workstation.dev", plan: "ChatGPT Plus", createdAt: now - 86400 * 7, lastUsedAt: now - 86400 * 2, isActive: false, status: { kind: "usageLimit", detail: "Лимит этого окна исчерпан; профиль вернётся после сброса." }, usage: { primary: { usedPercent: 100, windowMinutes: 300, resetsAt: now + 3600 * 1 }, secondary: { usedPercent: 100, windowMinutes: 10080, resetsAt: now + 86400 * 5 }, updatedAt: now - 100, error: null } },
        ],
        settings: { startWithWindows: true, startInTray: true, soundEnabled: false, switchThreshold: 1, automaticProfileSwitching: true, refreshIntervalSeconds: 60, proxyEnabled: true, theme: previewTheme, visualEffectsEnabled: true },
        currentAccountId: "preview-alex", refreshedAt: now - 48,
        codexPreferences: initialPreferences, prompts: [], codexConnected: previewConnected, codexRunning: previewConnected,
        proxyAddress: "http://127.0.0.1:15789", proxyError: null, adminProtected: false, adminUnlocked: true,
      });
      if (new URLSearchParams(window.location.search).has("limit-alert")) {
        setLimitAlert({ accountId: "preview-robot", email: "codex@workstation.dev", detail: "Лимит этого окна исчерпан.", resetAt: now + 3600 });
      }
      return;
    }
    void load();
    let disposed = false;
    let unlisten: (() => void)[] = [];
    const updateVisibility = () => setWindowVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", updateVisibility);
    updateVisibility();
    void listen("dashboard-changed", () => {
      if (updateTimer.current !== null) window.clearTimeout(updateTimer.current);
      updateTimer.current = window.setTimeout(() => {
        updateTimer.current = null;
        if (disposed) return;
        void load(true);
      }, 180);
    }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen<string>("app-message", (event) => showToast("Сообщение приложения", event.payload, "info")).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen("request-exit", () => {
      sound("exitPrompt", soundEnabledRef.current);
      setModalOrigin("center");
      setModalSource(null);
      setModal("exit");
    }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen<AppRestoredEvent>("app-restored", (event) => {
      setWindowVisible(true);
      void invoke("play_lifecycle_sound", { name: "restore" }).catch(() => undefined);
      if (windowAnimationTimer.current) window.clearTimeout(windowAnimationTimer.current);
      const animateRestore = visualEffectsPreference.current && !prefersReducedMotion;
      const fromTray = event.payload?.source === "tray";
      setAnimatingWindow(animateRestore ? `window-restore${fromTray ? " window-from-tray" : ""}` : "");
      void load(true);
      windowAnimationTimer.current = window.setTimeout(() => setAnimatingWindow(""), animateRestore ? 430 : 0);
    }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen("tray-hidden", () => {
      setWindowVisible(false);
    }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen<string>("account-add-started", (event) => {
      setAddingAccount(true);
      setAddStatus(event.payload || "Ожидаем завершения входа в браузере…");
    }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen<LoginAuthInfo>("account-add-auth-info", (event) => {
      const info = event.payload;
      if (info.url) {
        setAuthUrl(info.url);
        if (pendingAuthCopy.current) {
          pendingAuthCopy.current = false;
          void writeClipboardText(info.url).then((copied) => {
            if (copied) {
              setAddStatus("Ссылка скопирована в буфер обмена. Код показан ниже.");
              showToast("Ссылка скопирована", "Откройте её вручную в нужном браузере.", "ok");
            } else {
              showToast("Не удалось скопировать ссылку", "Попробуйте нажать кнопку ещё раз.", "error");
            }
          });
        }
      }
      if (info.openError) showToast("Не удалось открыть приватное окно", info.openError, "warning");
      else if (info.userCode) {
        setAuthUserCode(info.userCode);
        setAddStatus("Код подтверждения готов. Введите его на странице авторизации.");
      }
    }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen<DashboardData>("account-add-complete", (event) => {
      const addedAccountId = event.payload.currentAccountId;
      applyDashboard(event.payload);
      setAddingAccount(false);
      setAddStatus("");
      setAuthUrl("");
      setAuthUserCode("");
      setManualAuthLink(false);
      pendingAuthCopy.current = false;
      sound("accountAdded", soundEnabledRef.current);
      const routeNeedsConnection = event.payload.settings.proxyEnabled && !event.payload.codexConnected;
      if (!routeNeedsConnection) {
        showToast("Аккаунт добавлен", "Проверяем его лимиты и доступность.");
      }
      if (addedAccountId) {
        setBusyState(`refresh-${addedAccountId}`);
        void invoke<DashboardData>("refresh_account", { accountId: addedAccountId })
          .then((dashboard) => {
            applyDashboard(dashboard);
            if (routeNeedsConnection) {
              showToast(
                "Аккаунт добавлен, маршрут не подключён",
                dashboard.proxyError || event.payload.proxyError || "Лимиты обновлены. Подключите Codex кнопкой на главном экране.",
                "warning",
              );
            } else {
              showToast("Лимиты аккаунта обновлены", "Показаны данные последней проверки.");
            }
          })
          .catch((error) => {
            if (routeNeedsConnection) {
              showToast(
                "Аккаунт добавлен, маршрут не подключён",
                `${event.payload.proxyError || "Подключите Codex кнопкой на главном экране."} Лимиты не обновлены: ${getErrorText(error)}`,
                "warning",
              );
            } else {
              showToast("Аккаунт добавлен, лимиты не обновлены", getErrorText(error), "warning");
            }
          })
          .finally(() => {
            if (busyRef.current === `refresh-${addedAccountId}`) setBusyState("");
          });
      }
    }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen<string>("account-add-error", (event) => {
      setAddingAccount(false);
      setAddStatus("");
      setAuthUrl("");
      setAuthUserCode("");
      setManualAuthLink(false);
      pendingAuthCopy.current = false;
      if (event.payload.includes("отменён")) showToast("Вход отменён", event.payload, "info");
      else showToast("Не удалось добавить аккаунт", event.payload, "error");
    }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen("duplicate-launch", () => showToast("Приложение уже запущено", "Открыли существующее окно. Второй экземпляр не запускался.", "info")).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    void listen("admin-locked", () => { void load(true); }).then((listener) => { if (disposed) listener(); else unlisten.push(listener); });
    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", updateVisibility);
      unlisten.forEach((dispose) => dispose());
      if (updateTimer.current !== null) window.clearTimeout(updateTimer.current);
      if (windowAnimationTimer.current !== null) window.clearTimeout(windowAnimationTimer.current);
      if (effectsTransitionTimer.current !== null) window.clearTimeout(effectsTransitionTimer.current);
    };
  }, []);

  const sorted = useMemo(() => sortAccounts(data?.accounts ?? [], accountSort, usageClock, accountSortReversed), [data?.accounts, accountSort, accountSortReversed, usageClock]);
  const visibleAccounts = useMemo(() => sorted.filter((account) => matchesAccountSearch(account, accountSearch)), [sorted, accountSearch]);
  const limitCandidates = useMemo(() => {
    if (!data || !limitAlert) return [];
    return data.accounts
      .filter((account) => {
        if (account.id === limitAlert.accountId || account.isActive) return false;
        const status = displayAccountStatus(account, usageClock);
        const quota = availableNow(account, usageClock);
        return status.kind === "healthy" && quota !== null && quota > 0;
      })
      .sort((a, b) => {
        const aQuota = availableNow(a, usageClock) ?? -1;
        const bQuota = availableNow(b, usageClock) ?? -1;
        if (aQuota !== bQuota) return bQuota - aQuota;
        return (nextResetAt(a, usageClock) ?? Number.MAX_SAFE_INTEGER) - (nextResetAt(b, usageClock) ?? Number.MAX_SAFE_INTEGER);
      });
  }, [data?.accounts, limitAlert?.accountId, usageClock]);
  useEffect(() => {
    if (!limitAlert || !data) return;
    const source = data.accounts.find((account) => account.id === limitAlert.accountId);
    if (!source || !source.isActive || displayAccountStatus(source, usageClock).kind !== "usageLimit") {
      setLimitAlert(null);
      setToast((current) => current?.title === "Лимит аккаунта исчерпан" ? null : current);
      return;
    }
    const nextDetail = source.status.detail || "Лимит этого профиля исчерпан.";
    const nextReset = nextResetAt(source, usageClock);
    if (limitAlert.detail !== nextDetail || limitAlert.resetAt !== nextReset || limitAlert.email !== source.email) {
      setLimitAlert((current) => current?.accountId === source.id
        ? { accountId: source.id, email: source.email, detail: nextDetail, resetAt: nextReset }
        : current);
    }
  }, [data, limitAlert, usageClock]);

  const active = data ? data.accounts.find((account) => account.id === data.currentAccountId) ?? null : null;
  const activeRemaining = active ? availableNow(active, usageClock) : null;
  const readyCount = (data?.accounts ?? []).filter((account) => {
    if (accountStatusInfo(account.status).permanent) return false;
    const primary = remaining(account.usage?.primary ?? null, usageClock);
    const secondary = remaining(account.usage?.secondary ?? null, usageClock);
    const known = [primary, secondary].filter((value): value is number => value !== null);
    return known.length > 0 && Math.min(...known) > 0;
  }).length;

  const action = useCallback(async <T,>(name: string, run: () => Promise<T>, successTitle: string, successDetail?: string) => {
    // State updates are asynchronous, so the rendered `busy` value alone is
    // not enough to serialize two clicks in the same event loop turn.
    if (busyRef.current || busy) return null;
    setBusyState(name);
    let timeoutId: number | null = null;
    try {
      // A hung Codex CLI/app-server must never leave every control disabled.
      // The native request continues in the background, while the UI regains
      // control and the next dashboard-changed event can reconcile its result.
      const timeout = new Promise<never>((_, reject) => {
        timeoutId = window.setTimeout(() => reject(new Error("Codex не ответил вовремя. Обновите данные позже.")), 30_000);
      });
      const result = await Promise.race([run(), timeout]);
      if (result && typeof result === "object" && "accounts" in result) applyDashboard(result as unknown as DashboardData);
      showToast(successTitle, successDetail, "ok");
      return result;
    } catch (error) {
      showToast("Не удалось выполнить действие", getErrorText(error), "error");
      return null;
    } finally {
      if (timeoutId !== null) window.clearTimeout(timeoutId);
      if (busyRef.current === name) setBusyState("");
    }
  }, [applyDashboard, busy, setBusyState, showToast]);

  const refresh = async () => {
    sound("dashboard-refresh", soundEnabled);
    await action("refresh", () => invoke<DashboardData>("refresh_all"), "Данные обновлены", "Проверены лимиты и состояние аккаунтов.");
  };

  const refreshAccount = async (account: Account) => {
    sound(`account-refresh:${account.id}`, soundEnabled);
    await action(
      `refresh-${account.id}`,
      () => invoke<DashboardData>("refresh_account", { accountId: account.id }),
      "Лимиты аккаунта обновлены",
      account.email,
    );
  };

  const addAccount = async (event?: React.MouseEvent<HTMLElement>) => {
    if (addingAccount || busy) return;
    sound("account-add", soundEnabled);
    pendingAuthCopy.current = false;
    setManualAuthLink(false);
    setAuthSource(sourceRectForEvent(event));
    setAddingAccount(true);
    setAuthUrl("");
    setAuthUserCode("");
    setAddStatus("Открываем официальный вход в браузере…");
    try {
      await invoke("start_add_account");
    } catch (error) {
      setAddingAccount(false);
      setManualAuthLink(false);
      setAddStatus("");
      setAuthUrl("");
      setAuthUserCode("");
      showToast("Не удалось открыть вход", getErrorText(error), "error");
    }
  };

  const addAccountPrivately = async () => {
    if (busy && !addingAccount) return;
    sound("authPrivate", soundEnabled);
    if (addingAccount) {
      if (!authUrl) {
        showToast("Ссылка ещё готовится", "Подождите пару секунд и повторите.", "info");
        return;
      }
      try {
        await invoke("open_auth_link_private", { url: authUrl });
        showToast("Открыт приватный вход", "Завершите авторизацию в приватном окне браузера.", "ok");
      } catch (error) {
        showToast("Не удалось открыть приватное окно", getErrorText(error), "error");
      }
      return;
    }
    setAddingAccount(true);
    setManualAuthLink(true);
    setAuthUrl("");
    setAuthUserCode("");
    setAddStatus("Открываем официальный вход в приватном окне…");
    try {
      await invoke("start_private_add_account");
    } catch (error) {
      setAddingAccount(false);
      setManualAuthLink(false);
      setAddStatus("");
      setAuthUrl("");
      setAuthUserCode("");
      showToast("Не удалось открыть приватный вход", getErrorText(error), "error");
    }
  };

  const copyAuthLink = async () => {
    sound("authCopy", soundEnabled);
    // The copy action must always be independent from account creation.  The
    // old implementation started the device-code CLI flow and returned the
    // shared `/codex/device` page (and some builds opened a browser as a side
    // effect).  Reuse an already visible official OAuth URL when a browser
    // login is in progress; otherwise ask Rust for a fresh PKCE URL.  No
    // browser is launched from this path.
    if (addingAccount && !authUrl) {
      // The CLI browser flow can take a moment to print its one-time URL. Do
      // not race it with a second login session; the auth-info listener below
      // copies the URL as soon as it arrives.
      pendingAuthCopy.current = true;
      showToast("Ссылка ещё готовится", "Скопируем её автоматически, как только официальный вход её выдаст.", "info");
      return;
    }
    let link = authUrl.startsWith("https://auth.openai.com/oauth/authorize?")
      ? authUrl
      : "";
    let startedManualSession = false;
    try {
      const reusedActiveLink = Boolean(link);
      if (!link) {
        link = await invoke<string>("prepare_auth_link");
        startedManualSession = true;
      }
      if (!link.startsWith("https://auth.openai.com/oauth/authorize?")) {
        throw new Error("Сервис входа вернул некорректную ссылку.");
      }
      const copied = await writeClipboardText(link);
      if (!copied) throw new Error("Не удалось скопировать ссылку. Выделите её вручную.");
      if (startedManualSession) {
        setManualAuthLink(true);
        setAuthUrl(link);
        setAuthUserCode("");
        setAddStatus("Уникальная ссылка скопирована. Откройте её вручную в нужном браузере, затем вставьте адрес возврата.");
      } else if (reusedActiveLink) {
        setAuthUrl(link);
      }
      pendingAuthCopy.current = false;
      showToast("Уникальная ссылка скопирована", "Откройте её вручную в нужном браузере. Браузер не запускается автоматически.", "ok");
    } catch (error) {
      if (startedManualSession) void invoke("cancel_add_account").catch(() => undefined);
      pendingAuthCopy.current = false;
      showToast("Не удалось скопировать ссылку", getErrorText(error), "error");
    }
  };

  const pasteAuthCallback = async () => {
    sound("authPaste", soundEnabled);
    if (busyRef.current) {
      showToast("Действие уже выполняется", "Дождитесь завершения текущей операции и повторите вставку.", "warning");
      return;
    }

    setBusyState("auth-callback");
    try {
      if (!navigator.clipboard?.readText) {
        showToast("Буфер обмена недоступен", "WebView2 не предоставил приложению доступ к буферу обмена.", "error");
        return;
      }

      let callbackUrl = "";
      try {
        callbackUrl = (await navigator.clipboard.readText()).trim();
      } catch (error) {
        showToast("Не удалось прочитать буфер обмена", getErrorText(error), "error");
        return;
      }

      if (!callbackUrl) {
        showToast("Буфер обмена пуст", "Скопируйте полный адрес возврата после авторизации и повторите вставку.", "warning");
        return;
      }

      let callbackPath = "";
      try {
        callbackPath = new URL(callbackUrl).pathname;
      } catch {
        showToast("В буфере нет ссылки возврата", "Скопируйте полный адрес 127.0.0.1:1455/auth/callback из адресной строки браузера.", "warning");
        return;
      }
      if (callbackPath !== "/auth/callback") {
        showToast("В буфере нет ссылки возврата", "После входа скопируйте полный адрес 127.0.0.1:1455/auth/callback из браузера.", "warning");
        return;
      }

      showToast("Проверяем ссылку возврата", "Завершаем вход и добавляем профиль…", "info");
      await invoke("complete_add_account_callback", { callbackUrl });
    } catch (error) {
      const rawError = typeof error === "string"
        ? error
        : error instanceof Error
          ? error.message
          : "";
      if (rawError.includes("Ссылка ручного входа истекла")) {
        setAddingAccount(false);
        setManualAuthLink(false);
        setAuthUrl("");
        setAuthUserCode("");
        setAddStatus("");
        pendingAuthCopy.current = false;
        showToast("Ссылка входа истекла", "Скопируйте новую ссылку авторизации и повторите вход.", "warning");
      } else {
        showToast("Не удалось завершить вход", getErrorText(error), "error");
      }
    } finally {
      if (busyRef.current === "auth-callback") setBusyState("");
    }
  };

  const cancelAddAccount = async () => {
    sound("account-cancel", soundEnabled);
    setAddStatus("Отменяем вход…");
    try {
      await invoke("cancel_add_account");
      // The direct PKCE flow has no child process that can emit a completion
      // event after cancellation.  Close the overlay immediately for both
      // flows; a late CLI error event is still safe and only becomes a toast.
      setAddingAccount(false);
      setManualAuthLink(false);
      setAuthUrl("");
      setAuthUserCode("");
      setAddStatus("");
      pendingAuthCopy.current = false;
    } catch (error) {
      showToast("Не удалось отменить вход", getErrorText(error), "error");
    }
  };

  const toggleAddAccount = (event?: React.MouseEvent<HTMLElement>) => void (addingAccount || manualAuthLink ? cancelAddAccount() : addAccount(event));

  const switchAccount = async (account: Account) => {
    if (account.isActive) return false;
    const blockReason = manualSwitchBlockReason(account, Math.floor(Date.now() / 1000));
    if (blockReason) {
      showToast("Нельзя переключить профиль", blockReason, "warning");
      return false;
    }
    manualSwitch.current = true;
    try {
      const result = await action(`switch-${account.id}`, () => invoke<DashboardData>("switch_account", { accountId: account.id }), "Профиль выбран для новых чатов", "Текущий диалог останется на исходном профиле; остановленный запрос не будет перезапущен.");
      if (result) {
        setLimitAlert(null);
        setToast(null);
      }
      return !!result;
    } finally {
      manualSwitch.current = false;
    }
  };

  const openAccountSearchFromAlert = () => {
    setLimitAlert(null);
    setToast(null);
    window.requestAnimationFrame(() => {
      accountListRef.current?.scrollTo({ top: 0, behavior: "smooth" });
      accountSearchInput.current?.focus();
    });
  };

  const dismissLimitAlert = useCallback(() => {
    setLimitAlert(null);
    setToast(null);
  }, []);

  const openSettings = (event?: React.MouseEvent<HTMLElement>) => {
    if (!data) return;
    sound("settings", soundEnabled);
    setSettingsDraft({ ...data.settings });
    rememberModalSource("settings", event);
    setModal("settings");
  };

  const openCodexSettings = (event?: React.MouseEvent<HTMLElement>) => {
    if (!data) return;
    sound("codex", soundEnabled);
    setPreferencesDraft({ ...data.codexPreferences });
    setSelectedPrompt(data.prompts[0] ?? null);
    rememberModalSource("codex", event);
    setModal("codex");
  };

  const toggleSound = async () => {
    if (!data || busyRef.current || busy) return;
    const next = !soundEnabled;
    sound(next ? "soundOn" : "soundOff", true, true);
    setSoundEnabled(next);
    soundEnabledRef.current = next;
    const settings = { ...data.settings, soundEnabled: next };
    setBusyState("sound");
    try {
      const result = await invoke<DashboardData>("save_settings", { settings });
      if (disposedRef.current) return;
      applyDashboard(result);
      recordActivity(
        next ? "Звук включён" : "Звук выключен",
        next ? "Звуковые эффекты приложения включены." : "Звуковые эффекты приложения выключены.",
      );
    } catch (error) {
      showToast("Не удалось изменить звук", getErrorText(error), "error");
      setSoundEnabled(!next);
      soundEnabledRef.current = !next;
    } finally {
      if (busyRef.current === "sound") setBusyState("");
    }
  };

  const setTheme = async (theme: AppSettings["theme"]) => {
    if (!data || busy) return;
    sound("dashboard-theme", soundEnabled);
    await action("theme", () => invoke<DashboardData>("save_settings", { settings: { ...data.settings, theme } }), "Тема изменена", theme === "system" ? "Используется тема Windows." : theme === "dark" ? "Включена тёмная тема." : "Включена светлая тема.");
  };

  const toggleTheme = () => {
    if (!data) return;
    const next: AppSettings["theme"] = resolvedTheme === "dark" ? "light" : "dark";
    void setTheme(next);
  };

  const toggleVisualEffects = async () => {
    if (!data || busy) return;
    const enabled = !data.settings.visualEffectsEnabled;
    sound("effects", soundEnabled);
    setEffectsTransition(enabled ? "effects-arriving" : "effects-dissolving");
    if (effectsTransitionTimer.current) window.clearTimeout(effectsTransitionTimer.current);
    const result = await action("effects", () => invoke<DashboardData>("save_settings", { settings: { ...data.settings, visualEffectsEnabled: enabled } }), enabled ? "Визуальные эффекты включены" : "Визуальные эффекты выключены", enabled ? "Стекло, подсветка и анимации снова активны." : "Анимации и стеклянные эффекты выключены; рабочие состояния сохранены.");
    if (!result) {
      setEffectsTransition("");
      return;
    }
    effectsTransitionTimer.current = window.setTimeout(() => {
      effectsTransitionTimer.current = null;
      setEffectsTransition("");
    }, 620);
  };

  const saveSettings = async () => {
    if (!settingsDraft) return;
    const effectSettingChanged = !!data && settingsDraft.visualEffectsEnabled !== data.settings.visualEffectsEnabled;
    if (effectSettingChanged) {
      setEffectsTransition(settingsDraft.visualEffectsEnabled ? "effects-arriving" : "effects-dissolving");
      if (effectsTransitionTimer.current) window.clearTimeout(effectsTransitionTimer.current);
    }
    sound("settingsSave", soundEnabled);
    const result = await action("settings", () => invoke<DashboardData>("save_settings", { settings: settingsDraft }), "Настройки сохранены", `Лимиты проверяются каждые ${formatInterval(settingsDraft.refreshIntervalSeconds)}.`);
    if (result) {
      if (effectSettingChanged) {
        const effectsNowEnabled = settingsDraft.visualEffectsEnabled;
        recordActivity(
          effectsNowEnabled ? "Визуальные эффекты включены" : "Визуальные эффекты выключены",
          effectsNowEnabled
            ? "Стекло, подсветка и анимации включены."
            : "Анимации и стеклянные эффекты выключены; рабочие состояния сохранены.",
        );
      }
      setModal(null);
      if (effectSettingChanged) effectsTransitionTimer.current = window.setTimeout(() => {
        effectsTransitionTimer.current = null;
        setEffectsTransition("");
      }, 620);
    } else if (effectSettingChanged) setEffectsTransition("");
  };

  const unlockAdmin = async (password: string) => {
    sound("unlock", soundEnabledRef.current);
    const result = await action("unlock", () => invoke<DashboardData>("unlock_admin", { password }), "Доступ открыт", "Аккаунты и настройки доступны в этой сессии.");
    return !!result;
  };

  const configureAdminPassword = async (currentPassword: string, newPassword: string) => {
    sound("securitySave", soundEnabled);
    const result = await action("security", () => invoke<DashboardData>("configure_admin_password", {
      currentPassword: currentPassword || null,
      newPassword: newPassword || null,
    }), newPassword ? "Пароль сохранён" : "Защита отключена", newPassword ? "При следующем открытии потребуется пароль." : "Доступ к приложению снова открыт без пароля.");
    return !!result;
  };

  const resetSettings = async () => {
    sound("resetSettings", soundEnabled);
    const result = await action("reset-settings", () => invoke<DashboardData>("reset_settings_to_default"), "Настройки сброшены", "Аккаунты и настройки Codex сохранены.");
    if (result) {
      resetAccountViewPreferences();
      setModal(null);
    }
  };

  const resetApplication = async () => {
    sound("resetAll", soundEnabled);
    const result = await action("reset-full", () => invoke<DashboardData>("reset_application"), "Приложение сброшено", "Удалены аккаунты, защита паролем, управляемые настройки Codex и prompts.");
    if (result) {
      resetAccountViewPreferences();
      setEvents([]);
      setModal(null);
    }
  };

  const resetAccountViewPreferences = () => {
    setAccountColumns(2);
    setAccountSort("remaining");
    setAccountSortReversed(false);
    try { window.localStorage.removeItem(accountViewStorageKey); } catch { /* Local storage can be disabled in a locked-down profile. */ }
  };

  const connectCodex = async () => {
    sound("codex-connect", soundEnabled);
    const result = await action("connect", () => invoke<DashboardData>("connect_codex"), "Маршрут Codex настроен", data?.codexRunning ? "Перезапустите Codex один раз, чтобы активировать открытый маршрут." : "Откройте Codex после подключения.");
    if (result && data?.codexRunning) showToast("Нужен первый перезапуск Codex", "Это нужно только для первичного подключения. Дальше переключение идёт в открытой сессии.", "info");
  };

  const disconnectCodex = async () => {
    sound("disconnect", soundEnabled);
    const result = await action("disconnect", () => invoke<DashboardData>("disconnect_codex"), "Прокси отключён", "Перезапустите Codex, чтобы применить прямое подключение.");
    if (result) setModal(null);
  };

  const saveCodexPreferences = async () => {
    sound("codexSave", soundEnabled);
    const result = await action("codex-config", () => invoke<DashboardData>("save_codex_preferences", { preferences: preferencesDraft }), "Настройки Codex сохранены", "Общие параметры не зависят от выбранного аккаунта.");
    if (result) setModal(null);
  };

  const savePrompt = async (prompt: Prompt) => {
    sound("promptSave", soundEnabled);
    const result = await action("prompt", () => invoke<DashboardData>("save_prompt", { name: prompt.name, content: prompt.content }), "Prompt сохранён", `В Codex он доступен как /${prompt.name}.`);
    if (result) setSelectedPrompt(prompt);
  };

  const removePrompt = async (name: string) => {
    sound("promptDelete", soundEnabled);
    const result = await action("delete-prompt", () => invoke<DashboardData>("delete_prompt", { name }), "Prompt удалён", `/${name} больше не появится в Codex.`);
    if (result) setSelectedPrompt(result.prompts[0] ?? null);
  };

  const transferBackup = async (mode: "export" | "import", password: string) => {
    if (backupDialogOpen.current || busyRef.current) return;
    backupDialogOpen.current = true;
    const exporting = mode === "export";
    sound(exporting ? "backupExport" : "backupImport", soundEnabled);
    try {
      const filters = [{ name: t("Резервная копия Codex Accounts"), extensions: ["codexaccounts", "json"] }];
      const path = exporting
        ? await save({ title: t("Сохранить резервную копию"), defaultPath: "codex-accounts.codexaccounts", filters })
        : await open({ title: t("Выбрать резервную копию"), multiple: false, filters });
      if (typeof path !== "string") return;
      if (exporting) {
        await action("export", () => invoke("export_backup", { path, password: password || null }), "Копия сохранена", password ? "Аккаунты и настройки защищены паролем." : "Копия без пароля содержит токены входа. Храните файл в безопасном месте.");
      } else {
        const result = await action("import", () => invoke<DashboardData>("import_backup", { path, password: password || null }), "Копия импортирована", "Аккаунты, общие настройки и prompts восстановлены.");
        if (result) setModal(null);
      }
    } catch (error) {
      showToast("Не удалось выполнить действие", getErrorText(error), "error");
    } finally {
      backupDialogOpen.current = false;
    }
  };

  const exitApplication = async () => {
    if (lifecycleTimer.current !== null) window.clearTimeout(lifecycleTimer.current);
    if (lifecycleClearTimer.current !== null) window.clearTimeout(lifecycleClearTimer.current);
    markActiveButtonSoundEventHandled();
    const delay = data?.settings.visualEffectsEnabled === false || prefersReducedMotion ? 0 : 460;
    setAnimatingWindow(delay ? "window-exit" : "");
    lifecycleTimer.current = window.setTimeout(() => {
      lifecycleTimer.current = null;
      if (disposedRef.current) return;
      void action("exit", () => invoke("exit_application"), "Приложение закрывается", "Локальный маршрут Codex восстановлен.");
      if (delay) {
        lifecycleClearTimer.current = window.setTimeout(() => {
          lifecycleClearTimer.current = null;
          if (!disposedRef.current) setAnimatingWindow("");
        }, 320);
      }
    }, delay);
  };

  const removeAccount = (account: Account, event?: React.MouseEvent<HTMLElement>) => {
    if (busy) return;
    sound("deletePrompt", soundEnabled);
    rememberModalSource("account", event);
    setPendingRemove(account);
  };

  const confirmRemoveAccount = async () => {
    if (!pendingRemove || busy) return;
    sound("accountDelete", soundEnabled);
    const account = pendingRemove;
    const result = await action(`remove-${account.id}`, () => invoke<DashboardData>("remove_account", { accountId: account.id }), "Аккаунт удалён", "Данные входа удалены из локального хранилища.");
    if (result) setPendingRemove(null);
  };

  const minimize = async () => {
    if (lifecycleTimer.current !== null) window.clearTimeout(lifecycleTimer.current);
    if (lifecycleClearTimer.current !== null) window.clearTimeout(lifecycleClearTimer.current);
    lifecycleClearTimer.current = null;
    markActiveButtonSoundEventHandled();
    void invoke("play_lifecycle_sound", { name: "minimize" }).catch(() => undefined);
    const delay = data?.settings.visualEffectsEnabled === false || prefersReducedMotion ? 0 : 230;
    setAnimatingWindow(delay ? "window-leave" : "");
    lifecycleTimer.current = window.setTimeout(() => {
      lifecycleTimer.current = null;
      if (disposedRef.current) return;
      setAnimatingWindow("");
      setWindowVisible(false);
      void getCurrentWindow().minimize().catch((error) => {
        if (disposedRef.current) return;
        setWindowVisible(true);
        showToast("Не удалось свернуть окно", getErrorText(error), "error");
      });
    }, delay);
  };

  const toggleMaximize = async () => {
    if (nativeResizeCommand.current) return;
    sound("expand", soundEnabled);
    nativeResizeCommand.current = true;
    resizeInProgress.current = true;
    setAnimatingWindow("window-resize-live");
    document.documentElement.style.setProperty("--resize-backdrop", resolvedTheme === "light" ? "#faf7fd" : "#170d21");
    document.documentElement.classList.add("native-resizing-soft");
    document.body.classList.add("native-resizing-soft");
    if (nativeResizeGuardTimer.current !== null) window.clearTimeout(nativeResizeGuardTimer.current);
    nativeResizeGuardTimer.current = window.setTimeout(() => {
      nativeResizeGuardTimer.current = null;
      clearResizeState();
    }, 520);
    const currentWindow = getCurrentWindow();
    try {
      await currentWindow.toggleMaximize();
    } catch (error) {
      clearResizeState();
      showToast("Не удалось изменить размер окна", getErrorText(error), "error");
    }
  };

  const hideWindow = async () => {
    if (lifecycleTimer.current !== null) window.clearTimeout(lifecycleTimer.current);
    if (lifecycleClearTimer.current !== null) window.clearTimeout(lifecycleClearTimer.current);
    lifecycleClearTimer.current = null;
    markActiveButtonSoundEventHandled();
    void invoke("play_lifecycle_sound", { name: "hide" }).catch(() => undefined);
    const delay = data?.settings.visualEffectsEnabled === false || prefersReducedMotion ? 0 : 230;
    setAnimatingWindow(delay ? "window-leave" : "");
    lifecycleTimer.current = window.setTimeout(async () => {
      lifecycleTimer.current = null;
      if (disposedRef.current) return;
      setAnimatingWindow("");
      try {
        await invoke("hide_to_tray");
        if (disposedRef.current) return;
        setWindowVisible(false);
      } catch (error) {
        if (disposedRef.current) return;
        setWindowVisible(true);
        showToast("Не удалось свернуть приложение в трей", getErrorText(error), "error");
      }
    }, delay);
  };

  const stayInTray = () => {
    setModal(null);
    void hideWindow();
  };

  // Hooks must run in the same order while dashboard data is loading and
  // after it arrives. Keep pointer-glass cleanup above the loading return.
  const effectsEnabledForCleanup = Boolean(data?.settings.visualEffectsEnabled && windowVisible);
  useLayoutEffect(() => {
    if (effectsEnabledForCleanup) return;
    if (pointerFrame.current !== null) window.cancelAnimationFrame(pointerFrame.current);
    pointerFrame.current = null;
    pointerPosition.current?.target.style.setProperty("--pointer-opacity", "0");
    pointerPosition.current = null;
    deactivatePointerGlassSurface(pointerGlassSurface.current);
    pointerGlassSurface.current = null;
  }, [effectsEnabledForCleanup]);

  if (!data) {
    return <StartupScreen theme={systemTheme} error={toast?.tone === "error" ? toast.detail : undefined} soundEnabled={soundEnabledRef.current} onRetry={() => void load()} />;
  }

  if (data.adminProtected && !data.adminUnlocked) {
    return <AdminLockScreen theme={data.settings.theme === "system" ? systemTheme : data.settings.theme} visualEffectsEnabled={data.settings.visualEffectsEnabled && windowVisible} onUnlock={unlockAdmin} busy={busy} modal={modal} toast={toast} soundEnabled={soundEnabled} onStay={stayInTray} onExit={() => void exitApplication()} />;
  }

  const resolvedTheme = data.settings.theme === "system" ? systemTheme : data.settings.theme;
  // Native maximize/restore can emit a burst of resize events while WebView2
  // is still repainting.  Keep the preference intact, but suspend the costly
  // glass projection and layout transforms for those frames so the compositor
  // never presents stale copies of the dashboard.
  const windowIsResizing = animatingWindow === "window-resize-live" || !windowVisible;
  const visualEffectsPreferred = data.settings.visualEffectsEnabled && windowVisible;
  // Keep the user's visual-effects class stable while the native frame is
  // changing.  The resize CSS below disables only the expensive blur and
  // compositor transforms, avoiding a second flash from effects-on → off.
  const effectsEnabled = visualEffectsPreferred;
  const themeLabel = resolvedTheme === "dark"
    ? t("Переключить на светлую тему")
    : t("Переключить на тёмную тему");

  return (
    <MotionConfig reducedMotion={prefersReducedMotion ? "always" : "never"}>
    <motion.div className={`app-shell theme-${resolvedTheme} ${effectsEnabled ? "effects-on" : "effects-off"} ${windowVisible ? "" : "window-hidden"} ${animatingWindow} ${effectsTransition} ${languageTransition ? "language-transition" : ""}`} style={{ "--pointer-opacity": 0 } as React.CSSProperties} initial={false} animate={{ opacity: 1 }} transition={{ duration: effectsEnabled && !prefersReducedMotion ? 0.3 : 0, ease: [0.2, 0.8, 0.2, 1] }} onPointerDownCapture={beginButtonAnimationEvent} onPointerMove={(event) => {
      if (!windowVisible) return;
      if (!effectsEnabled || event.pointerType !== "mouse") {
        pointerPosition.current = null;
        deactivatePointerGlassSurface(pointerGlassSurface.current);
        pointerGlassSurface.current = null;
        event.currentTarget.style.setProperty("--pointer-opacity", "0");
        return;
      }
      const target = event.currentTarget;
      const rect = target.getBoundingClientRect();
      const pointerElement = event.target instanceof Element ? event.target : null;
      // Dropdown options keep their contained CSS hover state. Timer steppers
      // are standalone controls, so let their text receive the same pointer light.
      const scrollableSelector =
        ".account-list-scroll, .events-list, .modal-sheet, .limit-alert-candidates, .right-column, .choice-picker-menu, .account-sort-menu";
      const scrollableSurface = pointerElement?.closest<HTMLElement>(scrollableSelector);
      // Native WebView2 scrollbars are not always exposed as the pointer event
      // target. Fall back to the visible scroll hosts around the cursor so the
      // page-wide glass light does not paint a square over the scrollbar gutter.
      const scrollableCandidates = scrollableSurface
        ? [scrollableSurface]
        : Array.from(document.querySelectorAll<HTMLElement>(scrollableSelector));
      // Keep the sheet reflection away from the whole native scrollbar gutter.
      // WebView2 may report a scrollbar hover as the scroll host itself, so
      // leave a small safety inset around the custom rounded thumb as well.
      const pointerIsOnScrollbar = scrollableCandidates.some((surface) => {
        const rect = surface.getBoundingClientRect();
        const nearVerticalScrollbar = surface.scrollHeight > surface.clientHeight
          && event.clientX >= rect.right - Math.max(22, surface.offsetWidth - surface.clientWidth + 7)
          && event.clientX <= rect.right + 2
          && event.clientY >= rect.top + 8
          && event.clientY <= rect.bottom - 8;
        const nearHorizontalScrollbar = surface.scrollWidth > surface.clientWidth
          && event.clientY >= rect.bottom - Math.max(22, surface.offsetHeight - surface.clientHeight + 7)
          && event.clientY <= rect.bottom + 2
          && event.clientX >= rect.left + 8
          && event.clientX <= rect.right - 8;
        return nearVerticalScrollbar || nearHorizontalScrollbar;
      });
      if (pointerIsOnScrollbar) {
        pointerPosition.current = null;
        if (pointerFrame.current !== null) {
          window.cancelAnimationFrame(pointerFrame.current);
          pointerFrame.current = null;
        }
        deactivatePointerGlassSurface(pointerGlassSurface.current);
        pointerGlassSurface.current = null;
        target.style.setProperty("--pointer-opacity", "0");
        return;
      }
      const surface = pointerElement?.closest<HTMLElement>(
        ".overview-card, .account-card, .active-card, .events-card, .modal-sheet, button, .model-picker > input, .account-sort-option, .setting-row, .choice-picker-trigger, .interval-control, .interval-wheel-row",
      ) ?? null;
      pointerPosition.current = {
        target,
        x: event.clientX - rect.left,
        y: event.clientY - rect.top,
        surface,
        clientX: event.clientX,
        clientY: event.clientY,
      };
      if (pointerFrame.current !== null) return;
      pointerFrame.current = window.requestAnimationFrame(() => {
        pointerFrame.current = null;
        const position = pointerPosition.current;
        if (!position || !position.target.isConnected) return;
        position.target.style.setProperty("--pointer-x", `${position.x}px`);
        position.target.style.setProperty("--pointer-y", `${position.y}px`);
        position.target.style.setProperty("--pointer-opacity", "1");
        if (pointerGlassSurface.current !== position.surface) {
          deactivatePointerGlassSurface(pointerGlassSurface.current);
          pointerGlassSurface.current = position.surface;
          activatePointerGlassSurface(pointerGlassSurface.current);
        }
        const glassSurface = pointerGlassSurface.current;
        if (glassSurface?.isConnected) {
          const surfaceRect = glassSurface.getBoundingClientRect();
          const glassX = Math.max(0, Math.min(surfaceRect.width, position.clientX - surfaceRect.left));
          const glassY = Math.max(0, Math.min(surfaceRect.height, position.clientY - surfaceRect.top));
          const normalizedX = surfaceRect.width > 0 ? glassX / surfaceRect.width - 0.5 : 0;
          const normalizedY = surfaceRect.height > 0 ? glassY / surfaceRect.height - 0.5 : 0;
          glassSurface.style.setProperty("--glass-x", `${glassX}px`);
          glassSurface.style.setProperty("--glass-y", `${glassY}px`);
          const isSettingsControl = glassSurface.matches("button, .model-picker > input") && !!glassSurface.closest(".modal-sheet");
          const isDropdownControl = Boolean(glassSurface.closest(".choice-picker-menu, .account-sort-menu"));
          glassSurface.style.setProperty("--glass-tilt-x", isDropdownControl ? "0deg" : `${normalizedX * 1.4}deg`);
          glassSurface.style.setProperty("--glass-tilt-y", isDropdownControl ? "0deg" : `${normalizedY * -1.2}deg`);
          glassSurface.style.setProperty("--glass-stretch-x", String(isDropdownControl ? 1 : isSettingsControl ? 1.008 + normalizedX * 0.012 : 1.018 + normalizedX * 0.028));
          glassSurface.style.setProperty("--glass-stretch-y", String(isDropdownControl ? 1 : isSettingsControl ? 1.006 - normalizedY * 0.008 : 1.014 - normalizedY * 0.022));
        }
      });
    }} onPointerLeave={(event) => {
      pointerPosition.current = null;
      deactivatePointerGlassSurface(pointerGlassSurface.current);
      pointerGlassSurface.current = null;
      event.currentTarget.style.setProperty("--pointer-opacity", "0");
    }} onClickCapture={beginButtonSoundEvent} onClick={(event) => finishButtonSoundEvent(event, soundEnabledRef.current)}>
      <header className="titlebar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>
          <BrandMark />
          <div className="brand-copy"><b>Codex Accounts</b><small>{t("АККАУНТЫ · ЛИМИТЫ · ОБЩИЕ НАСТРОЙКИ")}</small></div>
        </div>
        <div className="titlebar-right">
          <div className="toolbar" aria-label={t("Основные действия")}>
            <ToolbarButton feedback="refresh" label="Обновить данные" onClick={() => void refresh()} disabled={!!busy} icon={<RefreshCw size={17} className={busy === "refresh" ? "spin" : ""} />} />
            <ToolbarButton feedback="backup" label="Резервная копия и экспорт" onClick={(event) => { sound("backup", soundEnabled); rememberModalSource("backup", event); setModal("backup"); }} icon={<HardDriveDownload size={17} />} disabled={!!busy} />
            <ToolbarButton feedback="settings" label="Настройки приложения" onClick={openSettings} icon={<Settings2 size={17} />} disabled={!!busy} />
            <ToolbarButton feedback="codex" label="Общие настройки Codex" onClick={openCodexSettings} icon={<SlidersHorizontal size={17} />} disabled={!!busy} />
            <ToolbarButton feedback="app-theme" className="theme-toggle" label={themeLabel} onClick={toggleTheme} icon={resolvedTheme === "dark" ? <Moon size={17} /> : <Sun size={17} />} disabled={!!busy} />
            <button className="toolbar-button toolbar-language" data-feedback="app-language" type="button" aria-label={languageSwitchHint(language === "ru" ? "en" : "ru")} title={languageSwitchHint(language === "ru" ? "en" : "ru")} onClick={() => { const next: Language = language === "ru" ? "en" : "ru"; triggerLanguageTransition(next); sound(next === "en" ? "dashboard-language-en" : "dashboard-language-ru", soundEnabled); }}><span>{language.toUpperCase()}</span></button>
            <ToolbarButton feedback="app-sound" className="sound-toggle" label={soundEnabled ? "Выключить звук" : "Включить звук"} onClick={() => void toggleSound()} icon={soundEnabled ? <Volume2 size={17} /> : <VolumeX size={17} />} active={soundEnabled} disabled={!!busy} />
            <ToolbarButton feedback="app-effects" className="effects-toggle" label={visualEffectsPreferred ? "Выключить визуальные эффекты" : "Включить визуальные эффекты"} onClick={() => void toggleVisualEffects()} icon={<Waves className="effects-toggle-icon" size={17} />} active={visualEffectsPreferred} disabled={!!busy} />
          </div>
          <span className="version-chip"><span className="live-dot" />{data?.codexConnected ? t("ЛОКАЛЬНЫЙ МАРШРУТ") : t("ГОТОВ К РАБОТЕ")}</span>
          <button className="window-control minimize" data-feedback="window-minimize" aria-label={t("Свернуть")} title={t("Свернуть")} onClick={() => void minimize()}><Minus size={15} /></button>
          <button className="window-control maximize" data-feedback="window-maximize" aria-label={t("Развернуть")} title={t("Развернуть или восстановить")} onClick={() => void toggleMaximize()}><span className="max-icon" /></button>
          <button className="window-control close" data-feedback="window-hide" aria-label={t("Свернуть в трей")} title={t("Свернуть в системный трей")} onClick={() => void hideWindow()}><X size={15} /></button>
        </div>
      </header>

      <main className="main-layout">
        <section className="left-column">
          <div className="welcome-row">
            <div>
              <p className="eyebrow"><span className="eyebrow-line" />{t("ПРОФИЛИ CODEX")}</p>
              <h1>{t("Лимиты аккаунтов")}</h1>
              <p className="lead">{t("Остаток, время сброса и активный профиль.")}</p>
            </div>
            <motion.section className="overview-card overview-card-inline" layout={!windowIsResizing && effectsEnabled}>
              <div className="overview-copy">
                <div className="overview-label"><Activity size={14} /> {' '}{t("ОБЗОР АККАУНТОВ")}</div>
                <div className="overview-stats">
                  <div className="overview-stat overview-stat-accounts"><strong>{data?.accounts.length ?? "—"}</strong><span>{accountCountLabel(data?.accounts.length ?? 0, language)}</span></div>
                  <span className="stat-divider" aria-hidden="true" />
                  <div className="overview-stat overview-stat-ready"><strong>{readyCount}</strong><span>{t("с доступным лимитом")}</span></div>
                  <span className="stat-divider stat-divider-updated" aria-hidden="true" />
                  <div className="overview-updated"><span>{t("Обновление данных")}</span><strong className="updated-value">{formatUpdated(data?.refreshedAt)}</strong></div>
                </div>
              </div>
              <div className={`overview-orbit ${data?.codexConnected ? "connected" : ""}`} aria-hidden="true">
                <div className="orbit-ring orbit-ring-one" /><div className="orbit-ring orbit-ring-two" />
                <div className="orbit-core"><Route size={23} strokeWidth={1.9} /></div><div className="orbit-point point-one" /><div className="orbit-point point-two" /><div className="orbit-point point-three" />
              </div>
            </motion.section>
          </div>

          <section className="accounts-section">
            <div className="section-heading">
              <div className="section-title"><h2>{t("Аккаунты")}</h2></div>
              <div className="account-heading-actions">
                <div className="account-view-toolbar" aria-label={t("Вид и сортировка аккаунтов")}>
                  <div className="account-search-slot is-open">
                    <label className="account-search-field account-search-field-static">
                      <Search size={13} aria-hidden="true" /><input ref={accountSearchInput} type="search" value={accountSearch} onChange={(event) => setAccountSearch(event.target.value)} placeholder={accountSearchPlaceholder} aria-label={t("Поиск аккаунтов по имени, почте или тарифу")} autoComplete="off" />{accountSearch && <button type="button" aria-label={t("Очистить поиск")} onClick={() => { setAccountSearch(""); accountSearchInput.current?.focus(); }}><X size={13} /></button>}
                    </label>
                  </div>
                  <div className="account-layout-switch" role="group" aria-label={t("Аккаунтов в строке")}>
                    <button type="button" data-feedback="columns-1" className={accountColumns === 1 ? "selected" : ""} aria-label={t("Один аккаунт в строке")} aria-pressed={accountColumns === 1} title={t("Один аккаунт в строке")} onClick={() => setAccountColumns(1)}><Rows3 size={14} /></button>
                    <button type="button" data-feedback="columns-2" className={accountColumns === 2 ? "selected" : ""} aria-label={t("Два аккаунта в строке")} aria-pressed={accountColumns === 2} title={t("Два аккаунта в строке")} onClick={() => setAccountColumns(2)}><Columns2 size={14} /></button>
                    <button type="button" data-feedback="columns-3" className={accountColumns === 3 ? "selected" : ""} aria-label={t("Три аккаунта в строке")} aria-pressed={accountColumns === 3} title={t("Три аккаунта в строке")} onClick={() => setAccountColumns(3)}><Columns3 size={14} /></button>
                  </div>
                    <AccountSortMenu value={accountSort} onChange={setAccountSort} reversed={accountSortReversed} onToggleDirection={() => setAccountSortReversed((current) => !current)} animated={effectsEnabled} />
                </div>
                <div className="account-auth-actions">
                  <button data-feedback={manualAuthLink ? "account-cancel" : "account-add"} className={`add-account ${addingAccount || manualAuthLink ? "add-account-waiting" : ""} ${manualAuthLink && !addingAccount ? "account-auth-cancel" : ""}`} onClick={toggleAddAccount} disabled={addingAccount || busy !== ""} title={t(addingAccount ? (manualAuthLink ? "Вход по коду" : "Вход в браузере") : manualAuthLink ? "Отменить вход" : "Добавить аккаунт")}>
                    {addingAccount ? <LoaderCircle className="spin" size={16} /> : manualAuthLink ? <XCircle size={16} /> : <Plus size={17} />}
                    <span>{addingAccount ? t(manualAuthLink ? "Вход по коду" : "Вход в браузере") : manualAuthLink ? t("Отменить вход") : t("Добавить аккаунт")}</span>
                  </button>
                  <div className="account-auth-tools" aria-label={t("Дополнительные способы входа")}>
                     <button className="account-auth-tool" data-feedback="auth-copy" type="button" title={t("Скопировать ссылку авторизации")} aria-label={t("Скопировать ссылку авторизации")} onClick={() => void copyAuthLink()}><Copy size={15} /></button>
                     <button className="account-auth-tool" data-feedback="auth-paste" type="button" title={t("Вставить ссылку возврата после авторизации")} aria-label={t("Вставить ссылку возврата после авторизации")} onClick={() => void pasteAuthCallback()}><ClipboardPaste size={15} /></button>
                  </div>
                </div>
              </div>
            </div>

            <div ref={accountListRef} className="account-list-scroll">
              {sorted.length === 0 ? <EmptyAccounts animated={effectsEnabled} /> : visibleAccounts.length === 0 ? <motion.div className="account-search-empty" role="status" initial={effectsEnabled ? { opacity: 0, y: 8 } : false} animate={{ opacity: 1, y: 0 }}><Search size={18} /><b>{t("Ничего не найдено")}</b><span>{t("Измените запрос или сбросьте поиск.")}</span><button type="button" onClick={() => setAccountSearch("")}>{t("Сбросить поиск")}</button></motion.div> : (
                <LayoutGroup>
                  <motion.div className={`account-list account-columns-${accountColumns}`} layout={!windowIsResizing && effectsEnabled} style={{ "--account-columns": accountColumns } as React.CSSProperties}>
                    <AnimatePresence initial={false}>
                      {visibleAccounts.map((account, index) => <AccountCard key={account.id} account={account} index={index} nowSeconds={usageClock} busy={busy} animateLayout={!windowIsResizing && effectsEnabled} animateEffects={effectsEnabled} onSwitch={() => void switchAccount(account)} onRefresh={() => void refreshAccount(account)} onRemove={(event) => void removeAccount(account, event)} onReauthenticate={() => void addAccount()} />)}
                    </AnimatePresence>
                  </motion.div>
                </LayoutGroup>
              )}
            </div>
          </section>
        </section>

        <aside className="right-column">
          <motion.section className="active-card" layout={!windowIsResizing && effectsEnabled}>
            <div className="card-topline"><p className="eyebrow">{t("ТЕКУЩИЙ СЕАНС")}</p><span className={`status-pill ${data?.codexConnected ? "status-connected" : "status-idle"}`}><i />{data?.codexConnected ? t("ПОДКЛЮЧЁН") : t("НЕ ПОДКЛЮЧЁН")}</span></div>
            {active ? <>
              <div className="active-account-info">
                <div className="avatar-large">{active.email.slice(0, 1).toUpperCase()}</div>
                <div><h2><EmailText email={active.email} /></h2><p className={isFreePlan(active.plan) ? "plan-free" : undefined}>{displayPlan(active.plan)}</p>{displayAccountStatus(active, usageClock).kind !== "healthy" && <span className={`active-account-status status-tone-${displayAccountStatus(active, usageClock).tone}`}><i />{displayAccountStatus(active, usageClock).label}</span>}</div>
              </div>
              <div className="active-quota">
                <div className="active-quota-head"><span>{t("ОСТАТОК АКТИВНОГО ЛИМИТА")}</span></div>
                <div className="active-quota-meter">
                  <div
                    className="active-quota-ring"
                    role="meter"
                    aria-label={t("Доступный лимит активного аккаунта")}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={activeRemaining ?? undefined}
                    style={{ "--quota-progress": `${activeRemaining ?? 0}%` } as React.CSSProperties}
                  >
                    <svg className="active-quota-svg" viewBox="0 0 100 100" aria-hidden="true">
                      <defs>
                        <linearGradient id="quota-ring-gradient" x1="0" y1="0" x2="1" y2="1">
                          <stop offset="0%" stopColor="var(--quota-ring-start)" />
                          <stop offset="100%" stopColor="var(--quota-ring-end)" />
                        </linearGradient>
                      </defs>
                      <circle className="quota-ring-track" cx="50" cy="50" r="43" pathLength="100" />
                      <motion.circle
                        className="quota-ring-value"
                        cx="50"
                        cy="50"
                        r="43"
                        pathLength="100"
                        strokeDasharray="100 100"
                        initial={false}
                        animate={{ strokeDashoffset: 100 - (activeRemaining ?? 0) }}
                        transition={{ duration: effectsEnabled ? .85 : 0, ease: [0.22, 0.61, 0.36, 1] }}
                      />
                    </svg>
                    <div className="active-quota-content">
                      <span className="active-quota-total">{activeRemaining === null ? "—" : <><b>{activeRemaining}</b><small>%</small></>}</span>
                      <small>{t("доступно")}</small>
                    </div>
                  </div>
                  <div className="active-quota-detail">
                    <span>{t("Ближайший сброс")}</span>
                    <b>{formatReset(nextResetAt(active, usageClock))}</b>
                    <small>{t("по самому короткому окну")}</small>
                    {activeRemaining !== null
                      && activeRemaining > 0
                      && activeRemaining <= (data?.settings.switchThreshold ?? 1)
                    && <small className="quota-warning-note">{t("Порог достигнут · смена вручную")}</small>}
                  </div>
                </div>
              </div>
            </> : <div className="active-empty"><div className="active-empty-icon"><KeyRound size={23} /></div><h2>{t("Выберите аккаунт")}</h2><p>{t("Добавьте профиль ChatGPT через официальный вход, чтобы следить за лимитами и переключаться между ними.")}</p><button className={`button-primary ${manualAuthLink && !addingAccount ? "account-auth-cancel" : ""}`} onClick={toggleAddAccount} disabled={busy !== "" && !addingAccount && !manualAuthLink}>{addingAccount || manualAuthLink ? <XCircle size={16} /> : <Plus size={16} />}{addingAccount || manualAuthLink ? t("Отменить вход") : t("Добавить аккаунт")}</button></div>}
            {data?.codexConnected ? <div className="connection-note"><span className="connection-light" /><div><b>{data.codexRunning ? t("Codex работает") : t("Прокси готов")}</b><small>{t("Новые чаты — с выбранного профиля; текущие закреплены за своим аккаунтом")}</small></div><span className="connection-mark"><Check size={17} /></span></div> : <div className="connect-callout"><div className="callout-icon"><TerminalSquare size={17} /></div><div><b>{t("Подключите Codex")}</b><small>{t("После подтверждённого исчерпания активного профиля приложение выберет доступный аккаунт с ближайшим сбросом. Остановленный запрос не повторяется.")}</small></div><button aria-label={t("Подключить Codex")} title={t("Подключить Codex")} onClick={() => void connectCodex()}><ArrowRight size={17} /></button></div>}
            {data?.proxyError && <div className="proxy-error"><CircleHelp size={15} /><span>{data.proxyError}</span></div>}
          </motion.section>

          <section className="events-card">
            <div className="events-heading"><div><p className="eyebrow">{t("ИСТОРИЯ ДЕЙСТВИЙ")}</p><h2>{t("События")}</h2></div><span className="event-count">{String(events.length)}</span></div>
            <div className="events-list">
              {events.length === 0 ? <div className="event-empty"><div className="event-empty-orbit"><History size={18} /></div><p>{t("События аккаунтов появятся здесь.")}</p><span>{t("Обновление лимитов · переключения · копии")}</span></div> : events.map((event) => <motion.div className={`event-row event-${event.tone}`} key={event.id} initial={effectsEnabled ? { opacity: 0, x: 9 } : false} animate={{ opacity: 1, x: 0 }} transition={{ duration: effectsEnabled ? 0.2 : 0 }}>
                <span className="event-time">{formatTime(event.at)}</span><span className="event-marker" /><div><b>{t(event.title)}</b><p>{t(event.detail)}</p></div>
              </motion.div>)}
            </div>
          </section>
          <div className="footer-note footer-version"><small>v0.1.0</small></div>
        </aside>
      </main>

      <AnimatePresence>
        {addingAccount && <AuthProgressOverlay
          status={t(addStatus)}
          theme={resolvedTheme}
          hasLink={!!authUrl}
          userCode={authUserCode}
          animate={effectsEnabled}
          source={authSource}
          onCancel={() => void cancelAddAccount()}
          onCopy={() => void copyAuthLink()}
          onPrivate={() => void addAccountPrivately()}
          onPaste={() => void pasteAuthCallback()}
        />}
      </AnimatePresence>

      <AnimatePresence>
        {toast && !limitAlert && <motion.div key={toast.id} role={toast.tone === "error" ? "alert" : "status"} className={`toast toast-${toast.tone} theme-${resolvedTheme}`} initial={effectsEnabled ? toastEntryMotion(toast.tone) : false} animate={{ opacity: 1, x: 0, y: 0, scale: 1, rotate: 0 }} exit={effectsEnabled ? toastExitMotion(toast.tone) : { opacity: 0, transition: { duration: 0 } }} transition={effectsEnabled ? { type: "spring", stiffness: 340, damping: 28, mass: .78 } : { duration: 0 }}>
          <div className={`toast-icon ${toast.tone === "ok" ? "toast-icon-glass-check" : ""}`}>{toast.tone === "error" ? <XCircle size={18} /> : toast.tone === "warning" ? <CircleHelp size={18} /> : toast.tone === "info" ? <Bell size={17} /> : <Check size={18} />}</div><div><b>{t(toast.title)}</b>{toast.detail && <p>{t(toast.detail)}</p>}</div><button className="toast-dismiss" aria-label={t("Закрыть уведомление")} onClick={() => setToast(null)}><X size={15} /></button>
        </motion.div>}
      </AnimatePresence>

      {createPortal(
        <div className={`limit-alert-portal theme-${resolvedTheme} ${effectsEnabled ? "effects-on" : "effects-off"}`}>
          <AnimatePresence>
            {limitAlert && !modal && !pendingRemove && <LimitAlertPanel alert={limitAlert} candidates={limitCandidates} search={accountSearch} busy={busy} nowSeconds={usageClock} animate={effectsEnabled} onSearch={setAccountSearch} onSwitch={(account) => void switchAccount(account)} onBrowse={openAccountSearchFromAlert} onDismiss={dismissLimitAlert} />}
          </AnimatePresence>
        </div>,
        document.body,
      )}

      <AnimatePresence mode="wait">
        {modal && <motion.div key={`modal-${modal}`} className={`modal-backdrop theme-${resolvedTheme} ${effectsEnabled ? "effects-on" : "effects-off"}`} initial={effectsEnabled ? { opacity: 0 } : false} animate={{ opacity: 1 }} exit={effectsEnabled ? { opacity: 0 } : { opacity: 0, transition: { duration: 0 } }} transition={{ duration: effectsEnabled ? 0.2 : 0 }} onMouseDown={(event) => { if (event.target === event.currentTarget) setModal(null); }}>
            <OriginMotionSection source={modalSource} kind={modalOrigin} animate={effectsEnabled} className={`modal-sheet modal-${modal} modal-source-${modalOrigin}`} onDismiss={() => setModal(null)}>
            <button className="modal-close" aria-label={t("Закрыть")} title={t("Закрыть")} onClick={() => setModal(null)}><X size={17} /></button>
            {modal === "settings" && settingsDraft && <SettingsPanel settings={settingsDraft} adminProtected={data.adminProtected} onChange={setSettingsDraft} onSave={() => void saveSettings()} onConnect={() => void connectCodex()} onDisconnect={() => void disconnectCodex()} onConfigureAdminPassword={configureAdminPassword} onResetSettings={() => setModal("reset-settings")} onFullReset={() => setModal("reset-full")} connected={data.codexConnected} running={data.codexRunning} busy={busy} />}
            {modal === "codex" && <CodexPanel preferences={preferencesDraft} prompts={data?.prompts ?? []} selectedPrompt={selectedPrompt} onPreferences={setPreferencesDraft} onSelectPrompt={setSelectedPrompt} onSavePreferences={() => void saveCodexPreferences()} onSavePrompt={(prompt) => void savePrompt(prompt)} onDeletePrompt={(name) => void removePrompt(name)} busy={busy} />}
            {modal === "backup" && <BackupPanel onExport={(password) => void transferBackup("export", password)} onImport={(password) => void transferBackup("import", password)} busy={busy} />}
            {modal === "exit" && <ExitPanel onStay={stayInTray} onExit={() => void exitApplication()} busy={busy === "exit"} />}
            {modal === "reset-settings" && <ResetPanel full={false} busy={busy === "reset-settings"} onCancel={() => setModal("settings")} onConfirm={() => void resetSettings()} />}
            {modal === "reset-full" && <ResetPanel full busy={busy === "reset-full"} onCancel={() => setModal("settings")} onConfirm={() => void resetApplication()} />}
            </OriginMotionSection>
        </motion.div>}
      </AnimatePresence>

      <AnimatePresence mode="wait">
        {pendingRemove && <motion.div key={`remove-${pendingRemove.id}`} className={`modal-backdrop theme-${resolvedTheme} ${effectsEnabled ? "effects-on" : "effects-off"}`} initial={effectsEnabled ? { opacity: 0 } : false} animate={{ opacity: 1 }} exit={effectsEnabled ? { opacity: 0 } : { opacity: 0, transition: { duration: 0 } }} transition={{ duration: effectsEnabled ? 0.2 : 0 }} onMouseDown={(event) => { if (event.target === event.currentTarget) setPendingRemove(null); }}>
          <OriginMotionSection source={modalSource} kind="account" animate={effectsEnabled} className="modal-sheet modal-remove modal-source-account" onDismiss={() => setPendingRemove(null)}>
            <button className="modal-close" aria-label={t("Закрыть")} title={t("Закрыть")} onClick={() => setPendingRemove(null)}><X size={17} /></button>
            <RemoveAccountPanel account={pendingRemove} busy={busy === `remove-${pendingRemove.id}`} onCancel={() => setPendingRemove(null)} onConfirm={() => void confirmRemoveAccount()} />
          </OriginMotionSection>
        </motion.div>}
      </AnimatePresence>
    </motion.div>
    </MotionConfig>
  );
}

function StartupScreen({ theme, error, soundEnabled, onRetry }: { theme: "dark" | "light"; error?: string; soundEnabled: boolean; onRetry: () => void }) {
  return <motion.main className={`app-shell startup-screen theme-${theme} effects-off`} initial={false} animate={{ opacity: 1 }} transition={{ duration: .32, ease: [0.2, 0.8, 0.2, 1] }} onClickCapture={beginButtonSoundEvent} onClick={(event) => finishButtonSoundEvent(event, soundEnabled)}>
    <BrandMark />
    <div className="startup-copy"><p className="eyebrow"><span className="eyebrow-line" />CODEX ACCOUNTS</p><h1>{error ? t("Данные пока не загрузились.") : t("Подготавливаем рабочее место.")}</h1><p>{error ?? t("Проверяем локальное хранилище и состояние Codex…")}</p>
      {error && <button className="button-primary" onClick={onRetry}><RefreshCw size={15} />{t("Повторить загрузку")}</button>}
    </div>
  </motion.main>;
}

function AuthProgressOverlay({ status, hasLink, userCode, animate, theme, source, onCancel, onCopy, onPrivate, onPaste }: {
  status: string;
  hasLink: boolean;
  userCode: string;
  animate: boolean;
  theme: "dark" | "light";
  source: MotionSource;
  onCancel: () => void;
  onCopy: () => void;
  onPrivate: () => void;
  onPaste: () => void;
}) {
  return <motion.div className={`auth-progress-backdrop theme-${theme} ${animate ? "effects-on" : "effects-off"}`} role="presentation" initial={animate ? { opacity: 0 } : false} animate={{ opacity: 1 }} exit={animate ? { opacity: 0 } : { opacity: 0, transition: { duration: 0 } }} transition={{ duration: animate ? .18 : 0 }}>
    <OriginMotionSection source={source} kind="auth" animate={animate} className="auth-progress-card" labelledBy="auth-progress-title" onDismiss={onCancel}>
      <span className="auth-progress-orbit"><LoaderCircle className="spin" size={25} /></span>
      <p className="eyebrow"><span className="eyebrow-line" />{t("ОФИЦИАЛЬНЫЙ ВХОД")}</p>
      <h2 id="auth-progress-title">{t("Завершите вход")}</h2>
      <p className="auth-progress-status" role="status">{status ? t(status) : t("Ждём подтверждения авторизации…")}</p>
      {userCode && <div className="auth-device-code"><span>{t("Одноразовый код")}</span><code>{userCode}</code></div>}
      <div className="auth-progress-tools">
        <button type="button" className="button-secondary" onClick={onCopy}><Copy size={15} />{t("Скопировать ссылку")}</button>
        <button type="button" className="button-secondary" disabled={!hasLink} onClick={onPrivate}><EyeOff size={15} />{t("Открыть приватно")}</button>
        <button type="button" className="button-secondary" onClick={onPaste}><ClipboardPaste size={15} />{t("Вставить ссылку возврата")}</button>
      </div>
      <button type="button" className="auth-progress-cancel" onClick={onCancel}><XCircle size={18} />{t("Отменить вход")}</button>
      <small>{t("После отмены приложение снова будет доступно.")}</small>
    </OriginMotionSection>
  </motion.div>;
}

function TrayMenu() {
  const [busy, setBusy] = useState("");
  const [message, setMessage] = useState("");
  const [closing, setClosing] = useState(false);
  const closeTimer = useRef<number | null>(null);
  const refreshCloseTimer = useRef<number | null>(null);
  const [soundEnabled, setSoundEnabled] = useState(true);
  const soundEnabledRef = useRef(true);
  const [visualEffectsEnabled, setVisualEffectsEnabled] = useState(true);
  const [language, setLanguageState] = useState<Language>(() => getLanguage());
  const [themeMode, setThemeMode] = useState<AppSettings["theme"]>("system");
  const [systemTheme, setSystemTheme] = useState<"dark" | "light">(() =>
    window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark",
  );
  const resolvedTheme = themeMode === "system" ? systemTheme : themeMode;
  const [anchor, setAnchor] = useState<TrayMenuAnchor>({ side: "bottom", offsetPercent: 50, requestId: 0, generation: 0 });
  const [openRequestId, setOpenRequestId] = useState(0);
  const [openSequence, setOpenSequence] = useState(0);
  const [paintReady, setPaintReady] = useState(true);
  const latestRequestId = useRef(0);
  const openGeneration = useRef(0);
  const trayPointerFrame = useRef<number | null>(null);
  const trayPointerPosition = useRef<{ root: HTMLElement; surface: HTMLElement | null; clientX: number; clientY: number } | null>(null);
  const trayGlassSurface = useRef<HTMLElement | null>(null);

  const clearTrayPointerGlass = (root?: HTMLElement) => {
    if (trayPointerFrame.current !== null) window.cancelAnimationFrame(trayPointerFrame.current);
    trayPointerFrame.current = null;
    trayPointerPosition.current = null;
    deactivatePointerGlassSurface(trayGlassSurface.current);
    trayGlassSurface.current = null;
    root?.style.setProperty("--pointer-opacity", "0");
  };

  const trackTrayPointerGlass = (event: React.PointerEvent<HTMLElement>) => {
    const root = event.currentTarget;
    if (!visualEffectsEnabled || event.pointerType !== "mouse") {
      clearTrayPointerGlass(root);
      return;
    }
    const target = event.target instanceof Element ? event.target.closest<HTMLElement>("button:not(:disabled)") : null;
    trayPointerPosition.current = { root, surface: target, clientX: event.clientX, clientY: event.clientY };
    if (trayPointerFrame.current !== null) return;
    trayPointerFrame.current = window.requestAnimationFrame(() => {
      trayPointerFrame.current = null;
      const position = trayPointerPosition.current;
      if (!position || !position.root.isConnected) return;
      const rootRect = position.root.getBoundingClientRect();
      position.root.style.setProperty("--pointer-x", `${position.clientX - rootRect.left}px`);
      position.root.style.setProperty("--pointer-y", `${position.clientY - rootRect.top}px`);
      position.root.style.setProperty("--pointer-opacity", "1");
      if (trayGlassSurface.current !== position.surface) {
        deactivatePointerGlassSurface(trayGlassSurface.current);
        trayGlassSurface.current = position.surface;
        activatePointerGlassSurface(trayGlassSurface.current);
      }
      const surface = trayGlassSurface.current;
      if (!surface?.isConnected) return;
      const rect = surface.getBoundingClientRect();
      const x = Math.max(0, Math.min(rect.width, position.clientX - rect.left));
      const y = Math.max(0, Math.min(rect.height, position.clientY - rect.top));
      const normalizedX = rect.width > 0 ? x / rect.width - 0.5 : 0;
      const normalizedY = rect.height > 0 ? y / rect.height - 0.5 : 0;
      surface.style.setProperty("--glass-x", `${x}px`);
      surface.style.setProperty("--glass-y", `${y}px`);
      surface.style.setProperty("--glass-tilt-x", `${normalizedX * 1.4}deg`);
      surface.style.setProperty("--glass-tilt-y", `${normalizedY * -1.2}deg`);
      surface.style.setProperty("--glass-stretch-x", String(1.018 + normalizedX * 0.028));
      surface.style.setProperty("--glass-stretch-y", String(1.014 - normalizedY * 0.022));
    });
  };

  useEffect(() => {
    if (visualEffectsEnabled) return;
    clearTrayPointerGlass();
  }, [visualEffectsEnabled]);

  useEffect(() => () => clearTrayPointerGlass(), []);

  useEffect(() => {
    document.body.classList.add("tray-menu-body");
    document.documentElement.classList.add("tray-menu-html");
    document.documentElement.lang = language;
    let unlisten: (() => void) | null = null;
    let unlistenTheme: (() => void) | null = null;
    let disposed = false;
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const syncSettings = async (): Promise<AppSettings | null> => {
      try {
        const dashboard = await invoke<DashboardData>("load_dashboard");
        if (disposed) return null;
        setSoundEnabled(dashboard.settings.soundEnabled);
        soundEnabledRef.current = dashboard.settings.soundEnabled;
        setVisualEffectsEnabled(dashboard.settings.visualEffectsEnabled);
        setThemeMode(dashboard.settings.theme);
        const currentLanguage = getLanguage();
        setLanguage(currentLanguage);
        setLanguageState(currentLanguage);
        document.documentElement.lang = currentLanguage;
        return dashboard.settings;
      } catch { /* The popup still offers open and quit if the dashboard is locked. */ }
      return null;
    };
    const onLanguageStorage = (event: StorageEvent) => {
      if (event.key !== "codex-accounts.language" || (event.newValue !== "ru" && event.newValue !== "en")) return;
      const next = event.newValue as Language;
      setLanguage(next);
      setLanguageState(next);
      document.documentElement.lang = next;
    };
    const onSystemTheme = (event: MediaQueryListEvent) => setSystemTheme(event.matches ? "light" : "dark");
    window.addEventListener("storage", onLanguageStorage);
    media.addEventListener("change", onSystemTheme);
    void listen<AppSettings["theme"]>("tray-theme-update", (event) => {
      setThemeMode(event.payload);
    }).then((listener) => {
      if (disposed) listener();
      else unlistenTheme = listener;
    });
    // Keep the first native presentation behind the same settings load as
    // subsequent openings.  Otherwise a transparent WebView can briefly
    // paint the previous theme before React receives the current settings.
    const initialSettings = syncSettings();
    const isTauriWindow = "__TAURI_INTERNALS__" in window;
    void listen<TrayMenuAnchor>("tray-menu-anchor", (event) => {
      // The native popup is hidden while the current settings are loaded.
      // This prevents a light/dark frame from leaking through on repeated
      // tray opens and also makes rapid right-clicks deterministic.
      const requestId = event.payload.requestId;
      // The native side retries while a hidden WebView2 wakes up. Process a
      // request only once so retries cannot replay sound or open animation.
      if (requestId !== 0 && requestId <= latestRequestId.current) return;
      latestRequestId.current = Math.max(latestRequestId.current, requestId);
      setPaintReady(false);
      void (async () => {
        const settings = await syncSettings();
        if (disposed || latestRequestId.current !== requestId) return;
        if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
        closeTimer.current = null;
        if (refreshCloseTimer.current !== null) window.clearTimeout(refreshCloseTimer.current);
        refreshCloseTimer.current = null;
        sound("trayOpen", settings?.soundEnabled ?? soundEnabledRef.current);
        setAnchor(event.payload);
        setOpenRequestId(requestId);
        setOpenSequence((current) => current + 1);
        setClosing(false);
        setMessage("");
      })();
    }).then((listener) => {
      if (disposed) listener();
      else {
        unlisten = listener;
        // The native popup starts hidden. Register its event handler before
        // showing it so the first anchor update cannot be lost during mount.
        if (isTauriWindow) {
          void initialSettings.finally(() => {
            window.requestAnimationFrame(() => {
              if (disposed) return;
              void invoke<TrayMenuAnchor>("get_tray_menu_anchor").then((currentAnchor) => {
                if (disposed || currentAnchor.requestId === 0) return;
                // The first tray window is created hidden. Do not present it before
                // React has painted the stored anchor: doing so exposes a stale
                // transparent WebView frame and produces a visible blink.
                if (currentAnchor.generation >= openGeneration.current) {
                  openGeneration.current = currentAnchor.generation;
                  setAnchor(currentAnchor);
                }
                latestRequestId.current = Math.max(latestRequestId.current, currentAnchor.requestId);
                setOpenRequestId(currentAnchor.requestId);
                setClosing(false);
              }).catch((error) => {
                if (!disposed) setMessage(`Не удалось открыть меню. ${getErrorText(error)}`);
              });
            });
          });
        }
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
      unlistenTheme?.();
      window.removeEventListener("storage", onLanguageStorage);
      media.removeEventListener("change", onSystemTheme);
      if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
      if (refreshCloseTimer.current !== null) window.clearTimeout(refreshCloseTimer.current);
      refreshCloseTimer.current = null;
      document.body.classList.remove("tray-menu-body");
      document.documentElement.classList.remove("tray-menu-html");
    };
  }, []);

  useLayoutEffect(() => {
    if (!openRequestId) return;
    let active = true;
    setPaintReady(true);
    let frame = 0;
    let secondFrame = 0;
    frame = window.requestAnimationFrame(() => {
      secondFrame = window.requestAnimationFrame(() => {
        void invoke<TrayMenuAnchor>("present_tray_menu", { expectedRequestId: openRequestId }).then((presentation) => {
        if (!active) return;
        if (presentation.requestId !== openRequestId || latestRequestId.current !== openRequestId) {
          if (presentation.requestId > latestRequestId.current) {
            latestRequestId.current = presentation.requestId;
            setAnchor(presentation);
            setOpenRequestId(presentation.requestId);
          }
          return;
        }
        if (presentation.generation >= openGeneration.current) {
          openGeneration.current = presentation.generation;
          setAnchor(presentation);
        }
        setOpenRequestId(0);
        }).catch((error) => {
          if (active && latestRequestId.current === openRequestId) {
            setMessage(`Не удалось открыть меню. ${getErrorText(error)}`);
            setOpenRequestId(0);
          }
        });
      });
    });
    return () => {
      active = false;
      window.cancelAnimationFrame(frame);
      window.cancelAnimationFrame(secondFrame);
    };
  }, [openRequestId]);

  const close = () => {
    markActiveButtonSoundEventHandled();
    sound("trayClose", soundEnabledRef.current);
    setClosing(true);
    if (refreshCloseTimer.current !== null) window.clearTimeout(refreshCloseTimer.current);
    refreshCloseTimer.current = null;
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null;
      void invoke("hide_tray_menu", { expectedGeneration: openGeneration.current }).catch(() => undefined);
    }, visualEffectsEnabled ? 200 : 0);
  };
  const openMain = () => {
    markActiveButtonSoundEventHandled();
    sound("trayOpenMain", soundEnabledRef.current);
    setClosing(true);
    if (refreshCloseTimer.current !== null) window.clearTimeout(refreshCloseTimer.current);
    refreshCloseTimer.current = null;
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null;
      void invoke("show_main_window").catch(() => undefined);
    }, visualEffectsEnabled ? 200 : 0);
  };
  const refresh = async () => {
    if (busy) return;
    if (refreshCloseTimer.current !== null) window.clearTimeout(refreshCloseTimer.current);
    refreshCloseTimer.current = null;
    sound("tray-refresh", soundEnabled);
    setBusy("refresh");
    setMessage("Проверяем лимиты…");
    try {
      await invoke("refresh_all");
      setMessage("Лимиты обновлены");
      setBusy("");
      if (refreshCloseTimer.current !== null) window.clearTimeout(refreshCloseTimer.current);
      refreshCloseTimer.current = window.setTimeout(() => {
        refreshCloseTimer.current = null;
        close();
      }, 420);
    } catch (error) {
      setMessage(getErrorText(error));
      setBusy("");
    }
  };
  const exit = async () => {
    if (busy) return;
    markActiveButtonSoundEventHandled();
    setBusy("exit");
    setClosing(true);
    try {
      await invoke("tray_exit_application");
    } catch (error) {
      setClosing(false);
      setBusy("");
      setMessage(`Не удалось закрыть приложение. ${getErrorText(error)}`);
    }
  };
  return <main className={`tray-menu-shell theme-${resolvedTheme} tray-menu-anchor-${anchor.side} ${visualEffectsEnabled ? "effects-on" : "effects-off"}`} style={{ "--tray-anchor-offset": `${anchor.offsetPercent}%`, visibility: openRequestId !== 0 && !paintReady ? "hidden" : "visible" } as React.CSSProperties} onPointerDownCapture={beginButtonAnimationEvent} onPointerMove={trackTrayPointerGlass} onPointerLeave={(event) => clearTrayPointerGlass(event.currentTarget)} onClickCapture={beginButtonSoundEvent} onClick={(event) => finishButtonSoundEvent(event, soundEnabled)}>
    <section key={openSequence} className={`tray-menu-card ${closing ? "tray-menu-closing" : ""}`} role="menu" aria-label={t("Меню Codex Accounts")}>
      <header className="tray-menu-head"><BrandMark className="tray-menu-mark" /><div><b>Codex Accounts</b><small>{t("БЫСТРЫЕ ДЕЙСТВИЯ")}</small></div><div className="tray-menu-head-actions"><button className="tray-menu-close" type="button" aria-label={t("Закрыть меню")} title={t("Закрыть меню")} onClick={close}><X size={15} /></button></div></header>
      {message && <p className="tray-menu-message" role="status">{t(message)}</p>}
      <button className="tray-menu-action tray-menu-primary" data-feedback="tray-open" aria-label={t("Открыть приложение")} data-tooltip={t("Показать аккаунты и лимиты")} onClick={openMain}><span className="tray-menu-icon"><Code2 size={16} /></span><span><b>{t("Открыть приложение")}</b><small>{t("Показать аккаунты и лимиты")}</small></span><ArrowRight size={15} /></button>
      <button className="tray-menu-action tray-menu-refresh" data-feedback="tray-refresh" data-tooltip={t("Проверить все профили сейчас")} onClick={() => void refresh()} disabled={!!busy}><span className="tray-menu-icon"><RefreshCw size={16} className={busy === "refresh" ? "spin" : ""} /></span><span><b>{t("Обновить лимиты")}</b><small>{t("Проверить все профили сейчас")}</small></span><ArrowRight size={15} /></button>
      <button className="tray-menu-action tray-menu-exit" data-feedback="tray-exit" data-tooltip={t("Завершить работу приложения")} onClick={exit} disabled={!!busy}><span className="tray-menu-icon"><X size={16} /></span><span><b>{t("Выйти")}</b><small>{t("Завершить работу приложения")}</small></span><ArrowRight size={15} /></button>
    </section>
  </main>;
}

function AdminLockScreen({ theme, visualEffectsEnabled, onUnlock, busy, modal, toast, soundEnabled, onStay, onExit }: { theme: "dark" | "light"; visualEffectsEnabled: boolean; onUnlock: (password: string) => Promise<boolean>; busy: string; modal: Modal; toast: Toast | null; soundEnabled: boolean; onStay: () => void; onExit: () => void }) {
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [error, setError] = useState("");
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setError("");
    if (await onUnlock(password)) setPassword("");
    else setError(t("Проверьте пароль и попробуйте ещё раз."));
  };
  return <motion.main className={`app-shell lock-screen theme-${theme} ${visualEffectsEnabled ? "effects-on" : "effects-off"}`} initial={visualEffectsEnabled ? { opacity: 0, scale: 0.985 } : false} animate={{ opacity: 1, scale: 1 }} transition={{ duration: visualEffectsEnabled ? 0.28 : 0 }} onPointerDownCapture={beginButtonAnimationEvent} onClickCapture={beginButtonSoundEvent} onClick={(event) => finishButtonSoundEvent(event, soundEnabled)}>
    <header className="titlebar"><div className="brand"><BrandMark /><div className="brand-copy"><b>Codex Accounts</b><small>{t("ЗАЩИЩЁННЫЙ РЕЖИМ")}</small></div></div>
      <button className="window-control close" data-feedback="window-hide" title={t("Свернуть в трей")} aria-label={t("Свернуть в трей")} onClick={() => void invoke("hide_to_tray")}><X size={15} /></button>
    </header>
    <section className="lock-card">
      <div className="lock-orbit"><LockKeyhole size={23} /></div><p className="eyebrow"><span className="eyebrow-line" />{t("ДОСТУП ЗАКРЫТ")}</p><h1>{t("Введите пароль")}</h1><p className="lock-description">{t("Аккаунты, лимиты и настройки скрыты до подтверждения доступа.")}</p>
      <form onSubmit={(event) => void submit(event)}>
        <label className="field-label">{t("Пароль администратора")}<div className="password-wrap"><input autoFocus autoComplete="current-password" type={showPassword ? "text" : "password"} value={password} onChange={(event) => setPassword(event.target.value)} /><button type="button" aria-label={showPassword ? t("Скрыть пароль") : t("Показать пароль")} onClick={() => setShowPassword(!showPassword)}>{showPassword ? <XCircle size={15} /> : <KeyRound size={15} />}</button></div></label>
        {error && <p className="lock-error"><CircleHelp size={14} />{t(error)}</p>}
        <button className="button-primary lock-submit" type="submit" disabled={!password || busy === "unlock"}>{busy === "unlock" ? <LoaderCircle className="spin" size={15} /> : <KeyRound size={15} />}{t("Открыть доступ")}</button>
      </form>
      <div className="lock-foot"><Shield size={13} />{t("Фоновая проверка лимитов продолжает работать.")}</div>
    </section>
    {modal === "exit" && <div className="modal-backdrop"><OriginMotionSection source={null} kind="center" animate={visualEffectsEnabled} className="modal-sheet modal-exit" onDismiss={onStay}><ExitPanel onStay={onStay} onExit={onExit} busy={busy === "exit"} /></OriginMotionSection></div>}
    <AnimatePresence>{toast && <motion.div className={`toast toast-${toast.tone} theme-${theme}`} initial={visualEffectsEnabled ? { opacity: 0, y: 18 } : false} animate={{ opacity: 1, y: 0 }} exit={visualEffectsEnabled ? { opacity: 0, y: 9 } : { opacity: 0, transition: { duration: 0 } }} transition={{ duration: visualEffectsEnabled ? 0.2 : 0 }}><div className="toast-icon">{toast.tone === "error" ? <XCircle size={18} /> : <Bell size={17} />}</div><div><b>{t(toast.title)}</b>{toast.detail && <p>{t(toast.detail)}</p>}</div></motion.div>}</AnimatePresence>
  </motion.main>;
}

function ResetPanel({ full, busy, onCancel, onConfirm }: { full: boolean; busy: boolean; onCancel: () => void; onConfirm: () => void }) {
  const [step, setStep] = useState(0);
  const [phrase, setPhrase] = useState("");
  const requiredPhrase = t("ПОЛНЫЙ СБРОС");
  return <div className="panel-content reset-content">
    <div className="modal-kicker"><Shield size={14} />{t("ПОДТВЕРЖДЕНИЕ СБРОСА")}</div>
    <div className="reset-indicator"><span className={step >= 0 ? "active" : ""} /><span className={step >= 1 ? "active" : ""} />{full && <span className={step >= 2 ? "active" : ""} />}</div>
    {step === 0 ? <>
      <h2>{full ? t("Полный сброс приложения?") : t("Вернуть настройки по умолчанию?")}</h2>
      <p className="modal-intro">{full ? t("Удалятся аккаунты, пароль администратора, временные данные, prompts и настройки Codex, которыми управляет приложение: модель, уровень рассуждений, доступ и общие инструкции. Автозапуск отключится, локальный маршрут восстановится. Остальные разделы config.toml сохранятся.") : t("Сбросятся настройки приложения, автозапуск, звук, интервал проверки и предупреждающий порог. Аккаунты, prompts и настройки Codex останутся.")}</p>
      {full && <div className="reset-warning"><CircleHelp size={15} /><span>{t("Если Codex сейчас работает через локальный маршрут, текущие запросы могут прерваться. Перезапустите Codex после сброса.")}</span></div>}
      <div className="reset-actions"><button className="button-secondary" onClick={onCancel}>{t("Отмена")}</button><button className="button-danger" onClick={() => setStep(1)}>{t("Продолжить")}</button></div>
    </> : full && step === 1 ? <>
      <h2>{t("Подтвердите удаление аккаунтов")}</h2><p className="modal-intro">{t("Данные аккаунтов нельзя восстановить без резервной копии. Введите фразу ниже, чтобы продолжить.")}</p>
      <label className="field-label">{t("Введите «")}{requiredPhrase}»<input value={phrase} onChange={(event) => setPhrase(event.target.value)} autoComplete="off" /></label>
      <div className="reset-actions"><button className="button-secondary" onClick={() => setStep(0)}>{t("Назад")}</button><button className="button-danger" disabled={phrase !== requiredPhrase} onClick={() => setStep(2)}>{t("Продолжить")}</button></div>
    </> : <>
      <h2>{full ? t("Последнее подтверждение") : t("Подтвердите сброс настроек")}</h2><p className="modal-intro">{full ? t("Программа останется открытой без сохранённых профилей и пароля. Основные настройки Codex и prompt-файлы будут удалены; другие пользовательские настройки config.toml сохранятся.") : t("Настройки приложения и вид списка аккаунтов вернутся к исходным значениям.")}</p>
      <div className="reset-actions"><button className="button-secondary" onClick={() => setStep(full ? 1 : 0)}>{t("Назад")}</button><button className="button-danger" disabled={busy} onClick={onConfirm}>{busy ? <LoaderCircle className="spin" size={15} /> : <Trash2 size={15} />}{full ? t("Выполнить полный сброс") : t("Сбросить настройки")}</button></div>
    </>}
  </div>;
}

function RemoveAccountPanel({ account, busy, onCancel, onConfirm }: { account: Account; busy: boolean; onCancel: () => void; onConfirm: () => void }) {
  return <div className="panel-content remove-content">
    <div className="modal-kicker"><Trash2 size={14} />{t("УДАЛЕНИЕ ПРОФИЛЯ")}</div>
    <h2>{t("Удалить аккаунт?")}</h2>
    <p className="modal-intro">{t("Профиль и сохранённые данные входа будут удалены из локального хранилища. Это действие нельзя отменить.")}</p>
    <div className="remove-preview">
      <div className="remove-avatar">{account.email.slice(0, 1).toUpperCase()}</div>
      <div className="remove-copy"><b><EmailText email={account.email} /></b><small className={isFreePlan(account.plan) ? "plan-free" : undefined}>{displayPlan(account.plan)}</small></div>
    </div>
    <div className="remove-actions"><button className="button-secondary" onClick={onCancel} disabled={busy}>{t("Отмена")}</button><button className="button-danger" onClick={onConfirm} disabled={busy}>{busy ? <LoaderCircle className="spin" size={14} /> : <Trash2 size={14} />}{t("Удалить аккаунт")}</button></div>
  </div>;
}

const accountSortOptions: Array<{ value: AccountSort; label: string; description: string }> = [
  { value: "remaining", label: "Больше лимита", description: "Сначала максимум доступного" },
  { value: "reset", label: "Ближе сброс", description: "Сначала ближайшее восстановление" },
  { value: "recent", label: "Последние активные", description: "Сначала недавно использованные" },
  { value: "name", label: "По имени", description: "По алфавиту" },
];

function AccountSortMenu({ value, onChange, reversed, onToggleDirection, animated }: { value: AccountSort; onChange: (value: AccountSort) => void; reversed: boolean; onToggleDirection: () => void; animated: boolean }) {
  const [open, setOpen] = useState(false);
  const opener = useRef<HTMLButtonElement | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const current = accountSortOptions.find((option) => option.value === value) ?? accountSortOptions[0];
  const currentLabel = t(current.label);
  const currentLabelRef = useRef<HTMLSpanElement | null>(null);
  useAdaptiveSingleLineFontSize(currentLabelRef, currentLabel, 12, 9);

  useEffect(() => {
    if (!open) return;
    const closeOnOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        opener.current?.focus();
      }
    };
    document.addEventListener("pointerdown", closeOnOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  const choose = (next: AccountSort) => {
    onChange(next);
    setOpen(false);
    opener.current?.focus();
  };

  const toggleMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    opener.current = event.currentTarget;
    setOpen((currentOpen) => !currentOpen);
  };
  const openMenuOnKey = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      opener.current = event.currentTarget;
      if (open) {
        rootRef.current?.querySelector<HTMLButtonElement>('[role="option"][aria-selected="true"]')?.focus();
        return;
      }
      setOpen(true);
    }
  };

  useLayoutEffect(() => {
    if (open) rootRef.current?.querySelector<HTMLButtonElement>('[role="option"][aria-selected="true"]')?.focus();
  }, [open]);

  const navigateOptions = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const options = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="option"]'));
    const current = options.findIndex((option) => option === document.activeElement);
    const last = options.length - 1;
    const next = event.key === "Home" ? 0
      : event.key === "End" ? last
      : event.key === "ArrowDown" ? (current + 1) % options.length
      : event.key === "ArrowUp" ? (current <= 0 ? last : current - 1)
      : null;
    if (next === null) return;
    event.preventDefault();
    options[next]?.focus();
  };

  return <div ref={rootRef} className={`account-sort-control ${open ? "is-open" : ""}`}>
    <button
      type="button"
      className={`account-sort-direction ${reversed ? "sort-direction-reversed" : ""}`}
      aria-label={t(reversed ? "Вернуть стандартное направление сортировки" : "Обратить порядок сортировки")}
      title={t(reversed ? "Вернуть стандартное направление сортировки" : "Обратить порядок сортировки")}
      aria-pressed={reversed}
      onClick={onToggleDirection}
    >
      <ArrowDownWideNarrow size={14} aria-hidden="true" />
    </button>
    <div className="account-sort-picker">
      <button
        type="button"
        className="account-sort-trigger"
        aria-label={`${t("Сортировка аккаунтов")}: ${currentLabel}`}
        title={`${t("Сортировка аккаунтов")}: ${currentLabel}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={toggleMenu}
        onKeyDown={openMenuOnKey}
      >
        <span ref={currentLabelRef}>{currentLabel}</span>
      </button>
      <button
        type="button"
        className="account-sort-menu-toggle"
        aria-label={t("Выбрать порядок сортировки")}
        title={t("Выбрать порядок сортировки")}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={toggleMenu}
        onKeyDown={openMenuOnKey}
      >
        <ChevronDown className="account-sort-chevron" size={14} aria-hidden="true" />
      </button>
    </div>
    <AnimatePresence>
      {open && <motion.div className="account-sort-menu" role="listbox" aria-label={t("Порядок аккаунтов")} onKeyDown={navigateOptions} initial={animated ? { opacity: 0, y: -6, scaleX: .72, scaleY: .34, borderRadius: "26px 26px 15px 15px" } : false} animate={{ opacity: 1, y: 0, scaleX: 1, scaleY: 1, borderRadius: "15px" }} exit={animated ? { opacity: 0, y: -4, scaleX: .82, scaleY: .25, borderRadius: "24px 24px 15px 15px" } : { opacity: 0, transition: { duration: 0 } }} transition={animated ? { type: "spring", stiffness: 500, damping: 27, mass: .56 } : { duration: 0 }}>
        {accountSortOptions.map((option, index) => <motion.button
          key={option.value}
          type="button"
          role="option"
          aria-selected={option.value === value}
          data-feedback={`sort-${option.value}`} className={`account-sort-option ${option.value === value ? "selected" : ""}`}
          initial={animated ? { opacity: 0, x: -8, scale: .96 } : false}
          animate={{ opacity: 1, x: 0, scale: 1 }}
          transition={animated ? { type: "spring", stiffness: 460, damping: 26, mass: .5, delay: index * .025 } : { duration: 0 }}
          whileTap={animated ? { scale: .97 } : undefined}
          onClick={() => choose(option.value)}
        >
          {option.value === value && <motion.span className="account-sort-active-pill" layoutId={animated ? "account-sort-active-pill" : undefined} transition={animated ? { type: "spring", stiffness: 520, damping: 31, mass: .52 } : { duration: 0 }} />}
          <span className="account-sort-option-copy"><b>{t(option.label)}</b></span>
          <Check className="account-sort-option-check" size={14} aria-hidden="true" />
        </motion.button>)}
      </motion.div>}
    </AnimatePresence>
  </div>;
}

function ToolbarButton({ label, onClick, icon, disabled = false, active = false, className = "", feedback }: { feedback?: string; label: string; onClick: (event: React.MouseEvent<HTMLButtonElement>) => void; icon: React.ReactNode; disabled?: boolean; active?: boolean; className?: string }) {
  return <button data-feedback={feedback ?? label} className={`toolbar-button ${active ? "toolbar-active" : ""} ${className}`.trim()} aria-label={t(label)} title={t(label)} aria-busy={disabled || undefined} onClick={onClick} disabled={disabled}>{icon}</button>;
}

/**
 * The logo is deliberately a single component rather than a collection of
 * slightly different images.  The same account-routing mark is used in the
 * app, setup, locked state and tray popup, while the small signal points are
 * only decorative motion and never move the artwork off its optical centre.
 */
function BrandMark({ installer = false, className = "" }: { installer?: boolean; className?: string }) {
  return <div className={`brand-mark brand-mark-visual${installer ? " installer-brand-mark" : ""}${className ? ` ${className}` : ""}`} aria-hidden="true">
    <span className="brand-mark-art"><ProductGlyph /></span>
  </div>;
}

function ProductGlyph() {
  return <svg className="product-glyph" viewBox="0 0 48 48" aria-hidden="true">
    <rect className="product-glyph-back" x="7" y="9" width="23" height="20" rx="6" />
    <path className="product-glyph-link" d="M12 33v1a6 6 0 0 0 6 6h8M33 10h2a5 5 0 0 1 5 5v2" />
    <rect className="product-glyph-front" x="19" y="19" width="22" height="20" rx="6" />
    <circle className="product-glyph-person" cx="30" cy="25.5" r="2.4" />
    <path className="product-glyph-person" d="M25 33a5 4 0 0 1 10 0Z" />
    <circle className="product-glyph-signal" cx="12" cy="33" r="2" />
  </svg>;
}

function EmptyAccounts({ animated }: { animated: boolean }) {
  return <motion.div className="empty-accounts" initial={animated ? { opacity: 0, y: 12 } : false} animate={{ opacity: 1, y: 0 }} transition={{ duration: animated ? 0.24 : 0 }}>
    <div className="empty-art"><div className="empty-art-circle"><KeyRound size={25} /></div><span className="empty-art-star">✳</span><span className="empty-art-line" /></div>
    <div><p className="eyebrow">{t("НАЧНЁМ С ОДНОГО")}</p><h3>{t("Аккаунты появятся здесь")}</h3><p>{t("Нажмите «Добавить аккаунт» вверху. Codex откроет официальный вход в браузере.")}</p></div>
  </motion.div>;
}

function AccountCard({ account, index, nowSeconds, busy, animateLayout = true, animateEffects, onSwitch, onRefresh, onRemove, onReauthenticate }: { account: Account; index: number; nowSeconds: number; busy: string; animateLayout?: boolean; animateEffects: boolean; onSwitch: () => void; onRefresh: () => void; onRemove: (event: React.MouseEvent<HTMLButtonElement>) => void; onReauthenticate: () => void }) {
  const p = remaining(account.usage?.primary ?? null, nowSeconds);
  const s = remaining(account.usage?.secondary ?? null, nowSeconds);
  const best = bestRemaining(account, nowSeconds);
  const status = displayAccountStatus(account, nowSeconds);
  const switchBlockReason = manualSwitchBlockReason(account, nowSeconds);
  return <motion.article className={`account-card ${account.isActive ? "account-active" : ""} account-status-${status.kind}`} layout={animateLayout} initial={animateLayout ? { opacity: 0, y: 16 } : false} animate={{ opacity: 1, y: 0 }} exit={animateLayout ? { opacity: 0, height: 0, marginBottom: 0, transition: { duration: 0.18 } } : { opacity: 0, transition: { duration: 0 } }} transition={{ layout: { type: "spring", stiffness: 285, damping: 28, mass: .78 }, opacity: { duration: .18 } }}>
    <div className="account-card-top">
      <div className="account-identity">
        <div className={`account-avatar avatar-${index % 4}`}><span>{account.email.slice(0, 1).toUpperCase()}</span><i /></div>
         <div className="account-name"><div><b><EmailText email={account.email} /></b>{account.isActive && <span className="current-tag"><span />{t("ТЕКУЩИЙ")}</span>}</div><small className="account-meta"><span className={`account-plan ${isFreePlan(account.plan) ? "plan-free" : ""}`}>{displayPlan(account.plan)}</span><span className="account-meta-sep">·</span><span className="account-updated">{account.usage ? `${t("Обновлено")} ${formatUpdated(account.usage.updatedAt)}` : t("Ожидает первой проверки")}</span></small></div>
      </div>
      <div className={`quota-badge tone-${toneFor(best)}`}><span className="quota-badge-label">{t("МАКСИМАЛЬНЫЙ ОСТАТОК")}</span><b>{best === null ? "—" : <>{best}<small>%</small></>}</b></div>
    </div>
     <div className="quota-windows">
      <QuotaLine title={formatWindowLabel(account.usage?.primary?.windowMinutes)} value={p} reset={account.usage?.primary?.resetsAt && account.usage.primary.resetsAt > nowSeconds ? account.usage.primary.resetsAt : null} animateEffects={animateEffects} />
      <QuotaLine title={formatWindowLabel(account.usage?.secondary?.windowMinutes)} value={s} reset={account.usage?.secondary?.resetsAt && account.usage.secondary.resetsAt > nowSeconds ? account.usage.secondary.resetsAt : null} animateEffects={animateEffects} />
    </div>
     {status.kind !== "healthy" && <div className={`account-status-note status-tone-${status.tone}`} role="status"><span className="account-status-marker" /><div><b>{t(status.label)}</b>{status.kind === "usageLimit" ? <small className="account-status-remedy">{t(status.hint)}</small> : <><small>{t(status.detail || status.hint)}</small>{status.detail && status.detail !== status.hint && <small className="account-status-remedy">{t(status.hint)}</small>}</>}</div>{status.action && <button type="button" onClick={onReauthenticate}>{t(status.action)}</button>}</div>}
      <div className="account-card-bottom">
      <span className="account-update">{account.usage?.error ? <><CircleHelp size={12} />{account.usage.error}</> : <><Activity size={12} />{account.usage ? `${t("Проверено")} ${formatTime(account.usage.updatedAt)}` : t("Лимиты ещё не проверялись")}</>}</span>
      <div className="account-actions">
        <button className="account-refresh" data-feedback="account-refresh" type="button" aria-label={`${t("Обновить лимиты")} ${account.email}`} title={t("Обновить лимиты этого аккаунта")} onClick={onRefresh} disabled={!!busy}><RefreshCw size={13} className={busy === `refresh-${account.id}` ? "spin" : ""} /></button>
        <button className="remove-account" data-feedback="account-remove" aria-label={`${t("Удалить")} ${account.email}`} title={t("Удалить аккаунт")} onClick={onRemove}><Trash2 size={14} /></button>
         <button data-feedback="account-switch" className={`switch-button ${account.isActive ? "is-current" : ""}`} aria-label={`${t("Переключить аккаунт")} ${account.email}`} onClick={onSwitch} disabled={account.isActive || !!busy || !!switchBlockReason} title={switchBlockReason ? t(switchBlockReason) : t("Переключить аккаунт")}>
          {busy === `switch-${account.id}` ? <LoaderCircle className="spin" size={14} /> : account.isActive ? <><Check size={14} /> {' '}{t("Выбран")}</> : <><ArrowRight size={14} />{t("Переключить")}</>}
        </button>
      </div>
    </div>
  </motion.article>;
}

function LimitAlertPanel({ alert, candidates, search, busy, nowSeconds, animate, onSearch, onSwitch, onBrowse, onDismiss }: {
  alert: LimitAlertData;
  candidates: Account[];
  search: string;
  busy: string;
  nowSeconds: number;
  animate: boolean;
  onSearch: (value: string) => void;
  onSwitch: (account: Account) => void;
  onBrowse: () => void;
  onDismiss: () => void;
}) {
  const visibleCandidates = candidates.filter((account) => matchesAccountSearch(account, search));
  const dialogRef = useRef<HTMLElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    searchRef.current?.focus({ preventScroll: true });
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onDismiss();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])',
      ) ?? []).filter((element) => element.offsetParent !== null);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true });
    };
  }, [onDismiss]);
  return <motion.div className="modal-backdrop limit-alert-backdrop" initial={animate ? { opacity: 0 } : false} animate={{ opacity: 1 }} exit={animate ? { opacity: 0 } : { opacity: 0, transition: { duration: 0 } }} transition={{ duration: animate ? .2 : 0 }} onMouseDown={(event) => { if (event.target === event.currentTarget) onDismiss(); }}>
    <motion.section ref={dialogRef} tabIndex={-1} className="limit-alert-sheet" role="alertdialog" aria-modal="true" aria-labelledby="limit-alert-title" aria-describedby="limit-alert-description" initial={animate ? { opacity: 0, y: 22, scale: .97 } : false} animate={{ opacity: 1, y: 0, scale: 1 }} exit={animate ? { opacity: 0, y: 14, scale: .98 } : { opacity: 0, transition: { duration: 0 } }} transition={animate ? { type: "spring", damping: 28, stiffness: 290 } : { duration: 0 }}>
      <button className="limit-alert-close" type="button" aria-label={t("Закрыть уведомление")} onClick={onDismiss}><X size={16} /></button>
      <div className="limit-alert-heading"><span className="limit-alert-symbol"><CircleHelp size={20} /></span><div><span className="modal-kicker">{t("ОСТАТОК ЛИМИТА")}</span><h2 id="limit-alert-title">{t("Лимит исчерпан")}</h2></div></div>
      <p className="limit-alert-description" id="limit-alert-description"><b>{alert.email}</b> · {t(alert.detail)}</p>
      <div className="limit-alert-reset"><Clock3 size={15} /><span>{t("Ближайший сброс")}</span><b>{formatReset(alert.resetAt)}</b></div>
      <div className="limit-alert-profiles-head"><div><b>{t("Выбрать профиль с доступным лимитом")}</b><span>{t("Автоматический переход недоступен")}</span></div><small>{visibleCandidates.length} {' '}{t("из")}{' '}{candidates.length}</small></div>
      <label className="limit-alert-search"><Search size={15} aria-hidden="true" /><input ref={searchRef} type="search" value={search} onChange={(event) => onSearch(event.target.value)} placeholder={t("Найти по имени или почте")} aria-label={t("Поиск профиля по имени или почте")} autoComplete="off" />{search && <button type="button" aria-label={t("Очистить поиск")} onClick={() => onSearch("")}><X size={14} /></button>}</label>
      <div className="limit-alert-candidates" role="list" aria-label={t("Профили для ручного выбора")}>
        {visibleCandidates.length ? visibleCandidates.map((account) => {
          const status = displayAccountStatus(account, nowSeconds);
          const quota = availableNow(account, nowSeconds);
          const primary = remaining(account.usage?.primary ?? null, nowSeconds);
          const secondary = remaining(account.usage?.secondary ?? null, nowSeconds);
          const canSwitch = !busy && !account.isActive && status.kind === "healthy" && quota !== null && quota > 0;
          const disabledLabel = account.isActive ? t("Уже выбран") : status.kind !== "healthy" ? status.label : quota === 0 ? t("Лимит исчерпан") : t("Данные не проверены");
          return <motion.article className={`limit-candidate ${canSwitch ? "limit-candidate-ready" : ""}`} role="listitem" layout key={account.id} initial={animate ? { opacity: 0, y: 8 } : false} animate={{ opacity: 1, y: 0 }} transition={{ duration: animate ? .18 : 0 }}>
            <span className="limit-candidate-avatar">{account.email.slice(0, 1).toUpperCase()}</span>
            <div className="limit-candidate-copy"><b title={account.email}>{account.email}</b><small className={isFreePlan(account.plan) ? "plan-free" : undefined}>{displayPlan(account.plan)}{account.usage ? <> · {t("проверено")} {formatUpdated(account.usage.updatedAt)}</> : t(" · лимиты не проверены")}</small><span>{primary === null ? t("Основной —") : <>{t("Основной")} {primary}%</>}<i />{secondary === null ? t("Недельный —") : <>{t("Недельный")} {secondary}%</>}</span></div>
            <div className="limit-candidate-action"><b>{quota === null ? "—" : <>{quota}%</>}</b><small>{account.isActive ? t("текущий") : t("доступно")}</small><button type="button" disabled={!canSwitch} onClick={() => onSwitch(account)}>{canSwitch ? busy === `switch-${account.id}` ? t("Переключение…") : t("Перейти") : disabledLabel}</button></div>
          </motion.article>;
        }) : <div className="limit-alert-empty"><Search size={17} /><b>{candidates.length ? t("Ничего не найдено") : t("Нет доступных профилей")}<small>{candidates.length ? t("Попробуйте изменить запрос.") : t("Обновите лимиты или восстановите вход в профилях с ошибкой.")}</small></b></div>}
      </div>
      <p className="limit-alert-footnote">{t("Профиль будет использоваться в новых чатах. Codex не перезапускается: текущий диалог и остановленный запрос остаются на исходном аккаунте. Общие настройки Codex сохраняются.")}</p>
      <div className="limit-alert-footer"><button type="button" className="limit-alert-browse" onClick={onBrowse}><Search size={14} />{t("Открыть полный список")}</button><button type="button" className="limit-alert-done" onClick={onDismiss}>{t("Понятно")}</button></div>
    </motion.section>
  </motion.div>;
}

function QuotaLine({ title, value, reset, animateEffects }: { title: string; value: number | null; reset: number | null; animateEffects: boolean }) {
  const tone = toneFor(value);
  const [pulse, setPulse] = useState(false);
  const previousValue = useRef<number | null | undefined>(undefined);

  useEffect(() => {
    const changed = previousValue.current !== undefined && previousValue.current !== value;
    previousValue.current = value;
    if (!animateEffects || !changed) {
      setPulse(false);
      return;
    }
    setPulse(true);
    const timer = window.setTimeout(() => setPulse(false), 850);
    return () => window.clearTimeout(timer);
  }, [value, animateEffects]);

  return <div className="quota-line">
    <div className="quota-line-label"><span>{title}</span><b className={`tone-text-${tone}`}>{value === null ? "—" : <span className="quota-value"><span>{value}</span><small>%</small></span>}</b></div>
    <div className={`quota-track tone-track-${tone} ${pulse ? "quota-track-pulse" : ""}`}><motion.span layout={animateEffects ? "position" : false} initial={false} animate={{ width: `${Math.max(0, value ?? 0)}%` }} transition={{ width: { duration: animateEffects ? .78 : 0, ease: [0.22, 0.61, 0.36, 1] } }} /></div>
     <div className="quota-line-reset"><span>{value === null ? t("Нет данных") : t("Сброс")}</span><b>{value === null ? t("Ожидается обновление") : formatReset(reset)}</b></div>
  </div>;
}

function ExitPanel({ onStay, onExit, busy }: { onStay: () => void; onExit: () => void; busy: boolean }) {
  return <div className="panel-content exit-panel">
    <div className="exit-orbit" aria-hidden="true"><TerminalSquare size={22} /></div>
    <div className="modal-kicker"><Shield size={14} /> {' '}{t("БЕЗОПАСНОЕ ЗАВЕРШЕНИЕ")}</div>
    <h2>{t("Закрыть Codex Accounts?")}</h2>
    <p className="modal-intro">{t("Сейчас открыт Codex. Если вы выйдете, выполняющийся запрос может прерваться. Менеджер восстановит исходный адрес маршрута в настройках Codex; чтобы продолжить работу, перезапустите Codex.")}</p>
      <div className="exit-actions"><button className="button-secondary" onClick={onStay}>{t("Остаться в трее")}</button><button className="button-danger" onClick={onExit} disabled={busy}>{busy ? <LoaderCircle className="spin" size={14} /> : <X size={14} />}{t("Выйти сейчас")}</button></div>
  </div>;
}

type WheelParts = [number, number, number];
type WheelSpinDirection = "up" | "down" | null;
type WheelSpinState = {
  directions: [WheelSpinDirection, WheelSpinDirection, WheelSpinDirection];
  cycles: WheelParts;
};
const wheelUnits = [
  { label: "часы" },
  { label: "минуты" },
  { label: "секунды" },
] as const;

function splitInterval(value: number): WheelParts {
  return [Math.floor(value / 3600), Math.floor((value % 3600) / 60), value % 60];
}

function intervalValue(parts: WheelParts) {
  return Math.max(1, Math.min(86_400, parts[0] * 3600 + parts[1] * 60 + parts[2]));
}

function wheelNeighbor(value: number, delta: number, index: number) {
  const maximum = index === 0 ? 24 : 59;
  if (index > 0) return (value + delta + 60) % 60;
  return Math.max(0, Math.min(maximum, value + delta));
}

function IntervalWheel({ value, onChange }: { value: number; onChange: (value: number) => void }) {
  const [parts, setParts] = useState<WheelParts>(() => splitInterval(value));
  const [drafts, setDrafts] = useState<[string, string, string]>(["", "", ""]);
  const [wheelSpin, setWheelSpin] = useState<WheelSpinState>({
    directions: [null, null, null],
    cycles: [0, 0, 0],
  });
  const previousValue = useRef(value);
  const fields = useRef<Array<HTMLInputElement | null>>([]);
  const advanceTimer = useRef<number | null>(null);

  useEffect(() => {
    const previousParts = splitInterval(previousValue.current);
    const nextParts = splitInterval(value);
    if (previousValue.current !== value) {
      const direction: Exclude<WheelSpinDirection, null> = value > previousValue.current ? "up" : "down";
      setWheelSpin((current) => {
        const directions = [...current.directions] as WheelSpinState["directions"];
        const cycles = [...current.cycles] as WheelParts;
        nextParts.forEach((part, index) => {
          if (part !== previousParts[index]) {
            directions[index] = direction;
            cycles[index] = (cycles[index] + 1) % 2;
          }
        });
        return { directions, cycles };
      });
    }
    previousValue.current = value;
    setParts(nextParts);
    setDrafts(["", "", ""]);
  }, [value]);

  useEffect(() => () => {
    if (advanceTimer.current !== null) window.clearTimeout(advanceTimer.current);
  }, []);

  const commit = (index: number, text: string, advance: boolean) => {
    if (advanceTimer.current !== null) window.clearTimeout(advanceTimer.current);
    advanceTimer.current = null;
    const maximum = index === 0 ? 24 : 59;
    const parsed = Number.parseInt(text, 10);
    if (Number.isFinite(parsed)) {
      const next = [...parts] as WheelParts;
      next[index] = Math.max(0, Math.min(maximum, parsed));
      const bounded = intervalValue(next);
      const normalized = splitInterval(bounded);
      setParts(normalized);
      onChange(bounded);
    }
    setDrafts((previous) => {
      const next = [...previous] as [string, string, string];
      next[index] = "";
      return next;
    });
    if (advance && index < 2) window.setTimeout(() => fields.current[index + 1]?.focus(), 0);
  };

  const step = (index: number, delta: number) => {
    if (index > 0 && parts[0] >= 24) return;
    const next = [...parts] as WheelParts;
    next[index] = wheelNeighbor(next[index], delta, index);
    const bounded = intervalValue(next);
    const normalized = splitInterval(bounded);
    setParts(normalized);
    setDrafts(["", "", ""]);
    onChange(bounded);
  };

  const typeValue = (index: number, raw: string) => {
    if (index > 0 && parts[0] >= 24) return;
    const digits = raw.replace(/\D/g, "").slice(-2);
    setDrafts((previous) => {
      const next = [...previous] as [string, string, string];
      next[index] = digits;
      return next;
    });
    if (advanceTimer.current !== null) window.clearTimeout(advanceTimer.current);
    if (!digits) {
      advanceTimer.current = null;
      return;
    }
    if (digits.length === 2) {
      commit(index, digits, true);
    } else {
      advanceTimer.current = window.setTimeout(() => commit(index, digits, true), 650);
    }
  };

  const wheelColumns = wheelUnits.map((unit, index) => {
    const current = parts[index];
    const locked = index > 0 && parts[0] >= 24;
    const spinDirection = wheelSpin.directions[index];
    const spinClass = spinDirection ? `wheel-roll-${spinDirection}-${wheelSpin.cycles[index]}` : "";
    return <div className={`interval-wheel-column ${locked ? "wheel-locked" : ""} ${spinClass}`} key={unit.label} onWheel={(event) => { event.preventDefault(); event.stopPropagation(); step(index, event.deltaY > 0 ? -1 : 1); }}>
      <label className="wheel-current">
        <input
          ref={(element) => { fields.current[index] = element; }}
          type="text"
          inputMode="numeric"
          autoComplete="off"
          maxLength={2}
          role="spinbutton"
          aria-valuemin={0}
          aria-valuemax={index === 0 ? 24 : 59}
          aria-valuenow={current}
          aria-valuetext={`${String(current).padStart(2, "0")} ${t(unit.label)}`}
          aria-disabled={locked}
          aria-label={index === 0 ? t("Часы интервала") : index === 1 ? t("Минуты интервала") : t("Секунды интервала")}
          value={drafts[index] || String(current).padStart(2, "0")}
          disabled={locked}
          onFocus={(event) => event.currentTarget.select()}
          onChange={(event) => typeValue(index, event.currentTarget.value)}
          onBlur={(event) => {
            if (drafts[index]) commit(index, drafts[index], false);
            event.currentTarget.setSelectionRange(0, 0);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commit(index, drafts[index] || String(current), index < 2);
            } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
              event.preventDefault();
              step(index, event.key === "ArrowUp" ? 1 : -1);
            } else if (event.key === "ArrowRight" && index < 2) {
              event.preventDefault();
              fields.current[index + 1]?.focus();
            } else if (event.key === "ArrowLeft" && index > 0) {
              event.preventDefault();
              fields.current[index - 1]?.focus();
            } else if (event.key === "Tab" && !event.shiftKey && index < 2) {
              event.preventDefault();
              commit(index, drafts[index] || String(current), true);
            } else if (event.key === "Tab" && event.shiftKey && index > 0) {
              event.preventDefault();
              commit(index, drafts[index] || String(current), false);
              fields.current[index - 1]?.focus();
            }
          }}
        />
        <span>{t(unit.label)}</span>
      </label>
    </div>;
  });

  return <section className="interval-control" aria-label={t("Интервал проверки лимитов")}>
    <div className="interval-control-head">
      <div className="interval-summary"><span>{t("Частота проверки лимитов")}</span></div>
      <b className="interval-readable">{formatInterval(value)}</b>
    </div>
    <div className="interval-wheel-row" role="group" aria-label={t("Установить часы, минуты и секунды")}>
      <div className="interval-wheel-control-row" role="group" aria-label={t("Уменьшить значение интервала")}>
        {wheelUnits.map((unit, index) => {
          const current = parts[index];
          const locked = index > 0 && parts[0] >= 24;
          const below = wheelNeighbor(current, -1, index);
          return <button key={`decrease-${unit.label}`} className="wheel-neighbor wheel-decrease" type="button" aria-label={`${t("Уменьшить")} ${t(unit.label)}`} title={`${t("Уменьшить")} ${t(unit.label)}`} disabled={locked} onClick={() => step(index, -1)}>{String(below).padStart(2, "0")}</button>;
        })}
      </div>
      <div className="interval-wheel-values" aria-label={t("Текущие часы, минуты и секунды")}>
        {wheelColumns}
      </div>
      <div className="interval-wheel-control-row" role="group" aria-label={t("Увеличить значение интервала")}>
        {wheelUnits.map((unit, index) => {
          const current = parts[index];
          const locked = index > 0 && parts[0] >= 24;
          const above = wheelNeighbor(current, 1, index);
          return <button key={`increase-${unit.label}`} className="wheel-neighbor wheel-increase" type="button" aria-label={`${t("Увеличить")} ${t(unit.label)}`} title={`${t("Увеличить")} ${t(unit.label)}`} disabled={locked} onClick={() => step(index, 1)}>{String(above).padStart(2, "0")}</button>;
        })}
      </div>
    </div>
  </section>;
}

function SettingsPanel({ settings, adminProtected, onChange, onSave, onConnect, onDisconnect, onConfigureAdminPassword, onResetSettings, onFullReset, connected, running, busy }: { settings: AppSettings; adminProtected: boolean; onChange: (next: AppSettings) => void; onSave: () => void; onConnect: () => void; onDisconnect: () => void; onConfigureAdminPassword: (currentPassword: string, newPassword: string) => Promise<boolean>; onResetSettings: () => void; onFullReset: () => void; connected: boolean; running: boolean; busy: string }) {
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [securityError, setSecurityError] = useState("");
  const set = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => onChange({ ...settings, [key]: value });
  const saveAdminPassword = async () => {
    setSecurityError("");
    if (!newPassword || newPassword !== confirmPassword) {
      setSecurityError(t("Введите новый пароль дважды одинаково."));
      return;
    }
    if (await onConfigureAdminPassword(currentPassword, newPassword)) {
      setCurrentPassword(""); setNewPassword(""); setConfirmPassword("");
    }
  };
  const disableAdminPassword = async () => {
    setSecurityError("");
    if (!currentPassword) { setSecurityError(t("Введите текущий пароль, чтобы отключить защиту.")); return; }
    if (await onConfigureAdminPassword(currentPassword, "")) {
      setCurrentPassword(""); setNewPassword(""); setConfirmPassword("");
    }
  };
  return <div className="panel-content">
    <div className="modal-kicker"><Settings2 size={14} /> {' '}{t("ПОВЕДЕНИЕ ПРИЛОЖЕНИЯ")}</div><h2>{t("Настройки")}</h2><p className="modal-intro">{t("Запуск, фоновые проверки и управление профилями.")}</p>
    <div className="settings-stack">
      <div className="settings-row-group">
      <SettingRow icon={<Command size={16} />} title={t("Автозапуск с Windows")} detail={t("Открывать менеджер после входа в систему")}><Toggle soundKey="startup-toggle" checked={settings.startWithWindows} onChange={(checked) => onChange({ ...settings, startWithWindows: checked, startInTray: checked ? settings.startInTray : false })} /></SettingRow>
      <SettingRow icon={<MoreHorizontal size={16} />} title={t("Сразу в системный трей")} detail={t("Работать тихо в фоне при запуске")}><Toggle soundKey="tray-toggle" checked={settings.startInTray} disabled={!settings.startWithWindows} onChange={(checked) => set("startInTray", checked)} /></SettingRow>
      <SettingRow icon={<RefreshCw size={16} />} title={t("Переключение профиля")} detail={t(settings.automaticProfileSwitching ? "После подтверждённого исчерпания активного профиля приложение выберет доступный аккаунт с ближайшим сбросом. Остановленный запрос не повторяется." : "Профиль выбирается вручную; каждый чат остаётся на своём аккаунте.")}><span className="setting-policy-badge">{t(settings.automaticProfileSwitching ? "Автоматически" : "Вручную")}</span><Toggle checked={settings.automaticProfileSwitching} soundKey="profile-switch-toggle" labelOverride={settings.automaticProfileSwitching ? "Переключить на ручной режим" : "Переключить на автоматический режим"} onChange={(checked) => set("automaticProfileSwitching", checked)} /></SettingRow>
      <SettingRow icon={<Waves size={16} />} title={t("Визуальные эффекты")} detail={t("Стекло, свечение и мягкие переходы")}><Toggle soundKey="effects-toggle" checked={settings.visualEffectsEnabled} onChange={(checked) => set("visualEffectsEnabled", checked)} /></SettingRow>
      </div>
      <div className="threshold-setting"><div><span>{t("Порог уведомления")}</span><b>{settings.switchThreshold}%</b></div><input aria-label={t("Порог уведомления об остатке лимита")} type="range" min="0" max="100" value={settings.switchThreshold} onChange={(event) => set("switchThreshold", Number(event.target.value))} /><small>{t("Предупреждение появится, когда доступный лимит опустится до этого значения.")}</small></div>
      <div className="settings-utility-grid">
        <IntervalWheel value={settings.refreshIntervalSeconds} onChange={(value) => set("refreshIntervalSeconds", value)} />
        <div className={`integration-box ${connected ? "integration-on" : ""}`}><div className="integration-icon"><TerminalSquare size={17} /></div><div><b>{connected ? t("Codex подключён к менеджеру") : t("Подключение к Codex")}</b><p>{connected ? (running ? t("Открытый Codex использует локальный маршрут.") : t("Маршрут заработает при запуске Codex.")) : t("Для первого подключения Codex нужно перезапустить один раз.")}</p></div>{connected ? <button className="text-button danger-text" onClick={onDisconnect} disabled={busy === "disconnect"}>{t("Отключить")}</button> : <button className="mini-primary" onClick={onConnect} disabled={busy === "connect"}>{busy === "connect" ? <LoaderCircle className="spin" size={15} /> : <><ArrowRight size={14} />{t("Подключить")}</>}</button>}</div>
      </div>
    </div>
    <section className="security-settings">
      <div className="security-heading"><Shield size={17} /><div><b>{t("Защита паролем")}</b><small>{adminProtected ? t("Включена · доступ блокируется при скрытии") : t("Необязательно · сейчас отключена")}</small></div></div>
      {adminProtected && <label className="field-label">{t("Текущий пароль")}<input type="password" autoComplete="current-password" value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} /></label>}
      <label className="field-label">{adminProtected ? t("Новый пароль") : t("Задайте пароль")}<input type="password" autoComplete="new-password" value={newPassword} onChange={(event) => setNewPassword(event.target.value)} placeholder={t("Не менее 8 символов")} /></label>
      <label className="field-label">{t("Повторите пароль")}<input type="password" autoComplete="new-password" value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} /></label>
      {securityError && <p className="lock-error">{t(securityError)}</p>}
      <div className="security-actions"><button className="mini-primary" onClick={() => void saveAdminPassword()} disabled={busy === "security"}>{busy === "security" ? <LoaderCircle className="spin" size={14} /> : <KeyRound size={14} />}{adminProtected ? t("Сменить пароль") : t("Включить защиту")}</button>{adminProtected && <button className="text-button danger-text" onClick={() => void disableAdminPassword()} disabled={busy === "security"}>{t("Отключить")}</button>}</div>
      <div className="reset-link-row"><button disabled={!!busy} onClick={onResetSettings}>{t("Вернуть настройки по умолчанию")}</button><button disabled={!!busy} onClick={onFullReset}>{t("Полный сброс приложения")}</button></div>
    </section>
    <div className="panel-footer"><span><Shield size={13} />{t("Настройки хранятся локально")}</span><button className="button-primary" onClick={onSave} disabled={!!busy}>{busy === "settings" ? <LoaderCircle className="spin" size={15} /> : <Check size={15} />}{t("Сохранить изменения")}</button></div>
  </div>;
}

function CodexPanel({ preferences, prompts, selectedPrompt, onPreferences, onSelectPrompt, onSavePreferences, onSavePrompt, onDeletePrompt, busy }: { preferences: CodexPreferences; prompts: Prompt[]; selectedPrompt: Prompt | null; onPreferences: (value: CodexPreferences) => void; onSelectPrompt: (prompt: Prompt | null) => void; onSavePreferences: () => void; onSavePrompt: (prompt: Prompt) => void; onDeletePrompt: (name: string) => void; busy: string }) {
  const [promptDraft, setPromptDraft] = useState<Prompt | null>(selectedPrompt);
  const [newPrompt, setNewPrompt] = useState(false);
  useEffect(() => setPromptDraft(selectedPrompt), [selectedPrompt]);
  const change = <K extends keyof CodexPreferences>(key: K, value: CodexPreferences[K]) => onPreferences({ ...preferences, [key]: value });
  const startNew = () => { const prompt = { name: "my-prompt", content: "" }; setPromptDraft(prompt); onSelectPrompt(prompt); setNewPrompt(true); };
  return <div className="panel-content codex-panel-content">
    <div className="modal-kicker"><SlidersHorizontal size={14} /> {' '}{t("ОБЩИЕ ПАРАМЕТРЫ CODEX")}</div><h2>{t("Настройки Codex")}</h2><p className="modal-intro">{t("Одинаковые при переключении аккаунта. Управляют тем, как Codex работает.")}</p>
    <div className="codex-fields">
      <ModelPicker value={preferences.model} onChange={(value) => change("model", value)} />
      <div className="codex-picker-field"><span className="field-label">{t("Уровень рассуждений")}</span><ChoicePicker value={preferences.modelReasoningEffort} onChange={(value) => change("modelReasoningEffort", value)} options={reasoningEffortOptions(preferences.model, preferences.modelReasoningEffort)} /></div>
      <div className="field-pair">
        <div className="codex-picker-field"><span className="field-label">{t("Доступ к файлам")}</span><ChoicePicker value={preferences.sandboxMode} onChange={(value) => change("sandboxMode", value)} options={[
          { value: "", label: "По умолчанию" },
          { value: "read-only", label: "Только чтение" },
          { value: "workspace-write", label: "Запись в рабочую папку" },
          { value: "danger-full-access", label: "Полный доступ" },
        ]} /></div>
        <div className="codex-picker-field"><span className="field-label">{t("Запрос подтверждения")}</span><ChoicePicker value={preferences.approvalPolicy} onChange={(value) => change("approvalPolicy", value)} options={[
          { value: "", label: "По умолчанию" },
          { value: "on-request", label: "Спрашивать при необходимости", detail: "Запрашивать разрешение, когда действие выходит за границы песочницы" },
          { value: "never", label: "Не спрашивать", detail: "Не показывать запросы подтверждения" },
        ]} /></div>
      </div>
      <label className="field-label">{t("Общая инструкция")}<textarea className="instruction-input" value={preferences.developerInstructions} onChange={(event) => change("developerInstructions", event.target.value)} placeholder={t("Например: отвечай по-русски, сначала изучай проект и объясняй важные изменения.")} /></label>
      <div className="settings-tip"><Shield size={14} /><span>{t("Уровни рассуждений зависят от модели. Режимы доступа и подтверждений действуют на все аккаунты, а доступ к файлам зависит от выбранной песочницы.")}</span></div>
      <div className="prompt-library">
        <div className="prompt-library-head"><div><span className="modal-kicker"><MessageSquareText size={13} /> {' '}{t("БИБЛИОТЕКА PROMPTS")}</span><small>{t("Markdown‑файлы для команды /имя")}</small></div><button className="small-icon-button" data-feedback="prompt-add" aria-label={t("Добавить prompt")} title={t("Добавить prompt")} onClick={startNew}><Plus size={16} /></button></div>
        {prompts.length === 0 && !newPrompt ? <div className="prompt-empty"><FilePlus2 size={17} /><span>{t("Добавьте повторяемые инструкции, например /review или /deploy.")}</span></div> : <div className="prompt-editor-row">
          <div className="prompt-list">{prompts.map((prompt) => <button className={promptDraft?.name === prompt.name ? "selected" : ""} key={prompt.name} onClick={() => { setPromptDraft(prompt); onSelectPrompt(prompt); setNewPrompt(false); }}><span>/{prompt.name}</span><ChevronDown size={13} /></button>)}{newPrompt && <div className="new-prompt-pill"><FilePlus2 size={13} />{t("Новый prompt")}</div>}</div>
          <div className="prompt-edit"><label className="field-label">{t("Команда")}<textarea value={promptDraft?.name ?? ""} onChange={(event) => setPromptDraft({ name: event.target.value.replace(/[^a-zA-Z0-9_-]/g, ""), content: promptDraft?.content ?? "" })} placeholder="review" /></label><label className="field-label">{t("Текст prompt")}<textarea className="prompt-body" value={promptDraft?.content ?? ""} onChange={(event) => setPromptDraft({ name: promptDraft?.name ?? "", content: event.target.value })} placeholder={t("Опишите повторяемую задачу…")} /></label><div className="prompt-actions">{selectedPrompt && !newPrompt && <button className="text-button danger-text" onClick={() => onDeletePrompt(selectedPrompt.name)}><Trash2 size={13} />{t("Удалить")}</button>}<button className="mini-primary" disabled={!promptDraft?.name || busy === "prompt"} onClick={() => promptDraft && onSavePrompt(promptDraft)}><Check size={14} />{newPrompt ? t("Создать prompt") : t("Сохранить")}</button></div></div>
        </div>}
      </div>
    </div>
    <div className="panel-footer"><span><Code2 size={13} />{t("Дополнительные ключи config.toml сохраняются")}</span><button className="button-primary" onClick={onSavePreferences} disabled={!!busy}>{busy === "codex-config" ? <LoaderCircle className="spin" size={15} /> : <Check size={15} />}{t("Сохранить настройки")}</button></div>
  </div>;
}

type PickerOption = { value: string; label: string; detail?: string };
type ModelOption = PickerOption & { reasoningEfforts: string[] };

function focusPickerOption(event: React.KeyboardEvent<HTMLElement>) {
  const options = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="option"]'));
  if (options.length === 0) return;
  const index = options.findIndex((option) => option === document.activeElement);
  if ((event.key === "Home" || event.key === "End") && index === -1) return;
  const next = event.key === "ArrowDown" ? (index + 1) % options.length
    : event.key === "ArrowUp" ? (index <= 0 ? options.length - 1 : index - 1)
    : event.key === "Home" ? 0
    : event.key === "End" ? options.length - 1
    : null;
  if (next === null) return;
  event.preventDefault();
  options[next].focus();
}

function ChoicePicker({ value, onChange, options }: { value: string; onChange: (value: string) => void; options: PickerOption[] }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement | null>(null);
  const selected = options.find((option) => option.value === value) ?? { value, label: value || "По умолчанию" };
  const selectedLabel = t(selected.label);
  const selectedLabelRef = useRef<HTMLSpanElement | null>(null);
  useAdaptiveSingleLineFontSize(selectedLabelRef, selectedLabel, 16, 14);
  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
    };
  }, [open]);
  const close = () => {
    root.current?.querySelector<HTMLButtonElement>(".choice-picker-trigger")?.focus();
    setOpen(false);
  };
  return <div className={`choice-picker ${open ? "is-open" : ""}`} ref={root} onKeyDown={(event) => {
    if (!open) {
      if (event.key === "ArrowDown") { event.preventDefault(); setOpen(true); }
      return;
    }
    if (event.key === "Escape") { event.stopPropagation(); close(); return; }
    focusPickerOption(event);
  }}>
    <button type="button" className="choice-picker-trigger" aria-haspopup="listbox" aria-expanded={open} title={t(open ? "Закрыть список вариантов" : "Открыть список вариантов")} onClick={() => setOpen((current) => !current)}>
      <span ref={selectedLabelRef}>{selectedLabel}</span>
    </button>
    <button type="button" className={`choice-picker-toggle ${open ? "is-open" : ""}`} aria-label={t(open ? "Закрыть список вариантов" : "Открыть список вариантов")} title={t(open ? "Закрыть список вариантов" : "Открыть список вариантов")} aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen((current) => !current)}>
      <ChevronDown size={15} aria-hidden="true" />
    </button>
    <AnimatePresence>
      {open && <motion.div className="choice-picker-menu" role="listbox" initial={{ opacity: 0, y: -5, scale: .985 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: -3, scale: .99 }} transition={{ duration: .14 }}>
        {options.map((option) => <button key={option.value || "default"} type="button" role="option" aria-selected={value === option.value} data-tooltip={[t(option.label), option.detail ? t(option.detail) : ""].filter(Boolean).join(" — ")} className={`choice-picker-option ${value === option.value ? "selected" : ""}`} onClick={() => { onChange(option.value); close(); }}>
          <span className="choice-picker-option-copy"><b>{t(option.label)}</b>{option.detail && <small>{t(option.detail)}</small>}</span>
          <Check size={15} />
        </button>)}
      </motion.div>}
    </AnimatePresence>
  </div>;
}

const modelOptions: ModelOption[] = [
  { value: "gpt-6-astra", label: "GPT-6 Astra", detail: "Для сложных задач и глубокого анализа", reasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
  { value: "gpt-6.1-sol", label: "GPT-6.1 Sol", detail: "Основная модель для разработки и повседневной работы", reasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
  { value: "gpt-6.1-terra", label: "GPT-6.1 Terra", detail: "Модель Codex для программирования", reasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
  { value: "gpt-6-sol", label: "GPT-6 Sol", detail: "Универсальная модель для сложных задач", reasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
  { value: "gpt-6-luna", label: "GPT-6 Luna", detail: "Быстрая модель для повседневных задач", reasoningEfforts: ["low", "medium", "high", "xhigh", "max"] },
  { value: "gpt-5.6-sol", label: "GPT-5.6 Sol", detail: "Предыдущее поколение универсальной модели", reasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
  { value: "gpt-5.6-terra", label: "GPT-5.6 Terra", detail: "Сбалансированная модель для работы с кодом", reasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"] },
  { value: "gpt-5.6-luna", label: "GPT-5.6 Luna", detail: "Быстрая и экономичная модель", reasoningEfforts: ["low", "medium", "high", "xhigh", "max"] },
  { value: "gpt-5.5", label: "GPT-5.5", detail: "Предыдущая модель для задач Codex", reasoningEfforts: ["low", "medium", "high", "xhigh"] },
];

function reasoningEffortOptions(model: string, currentValue: string): PickerOption[] {
  const normalized = model.trim().toLowerCase();
  const selectedModel = modelOptions.find((option) => option.value === normalized);
  const supportedEfforts = selectedModel?.reasoningEfforts ?? ["low", "medium", "high", "xhigh", "max"];
  const labels: Record<string, PickerOption> = {
    low: { value: "low", label: "Низкий", detail: "Быстрее для простых задач" },
    medium: { value: "medium", label: "Средний", detail: "Сбалансированный режим" },
    high: { value: "high", label: "Высокий", detail: "Для многошаговых задач" },
    xhigh: { value: "xhigh", label: "Очень высокий", detail: "Больше времени на сложную задачу" },
    max: { value: "max", label: "Максимальный", detail: "Максимальная глубина рассуждений" },
    ultra: { value: "ultra", label: "Ультра", detail: "Максимум с автоматическим делегированием задач" },
  };
  const options: PickerOption[] = [
    { value: "", label: "По умолчанию", detail: "Использовать настройку выбранной модели" },
    ...supportedEfforts.map((effort) => labels[effort]).filter((option): option is PickerOption => !!option),
  ];
  if (currentValue && !options.some((option) => option.value === currentValue)) {
    options.push({ value: currentValue, label: `Текущий уровень: ${currentValue}`, detail: "Значение сохранено в config.toml" });
  }
  return options;
}

function ModelPicker({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement | null>(null);
  const visible = modelOptions;
  const selectedModel = modelOptions.find((option) => option.value === value.trim().toLowerCase());
  // Keep known models human-readable even while the input is focused. Their
  // machine IDs are still passed through onChange and stored in config.toml.
  const displayValue = selectedModel ? t(selectedModel.label) : value;
  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
    };
  }, [open]);
  const close = () => {
    root.current?.querySelector<HTMLInputElement>("input")?.focus();
    setOpen(false);
  };
  return <div className="model-picker-field">
    <span className="field-label">{t("Модель")}</span>
    <div className={`model-picker ${open ? "is-open" : ""}`} ref={root} onKeyDown={(event) => {
      if (!open) {
        if (event.key === "ArrowDown") { event.preventDefault(); setOpen(true); }
        return;
      }
      if (event.key === "Escape") { event.stopPropagation(); close(); return; }
      focusPickerOption(event);
    }}>
      <input role="combobox" aria-label={t("Модель")} aria-expanded={open} aria-controls={open ? "codex-model-options" : undefined} value={displayValue} onFocus={() => setOpen(true)} onChange={(event) => { onChange(event.target.value); setOpen(true); }} placeholder={t("Модель Codex по умолчанию")} autoComplete="off" />
      <button type="button" className="model-picker-toggle" aria-label={open ? t("Свернуть список моделей") : t("Показать популярные модели")} title={t(open ? "Свернуть список моделей" : "Показать популярные модели")} onClick={() => setOpen((current) => !current)}><ChevronDown size={15} /></button>
      <AnimatePresence>
        {open && <motion.div className="choice-picker-menu model-picker-menu" id="codex-model-options" role="listbox" initial={{ opacity: 0, y: -5, scale: .985 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: -3, scale: .99 }} transition={{ duration: .14 }}>
          <div className="choice-picker-heading">{t("Популярные модели")}</div>
          <button type="button" className={`choice-picker-option ${value === "" ? "selected" : ""}`} role="option" aria-selected={value === ""} data-tooltip={`${t("Модель Codex по умолчанию")} — ${t("Выбор текущей версии Codex")}`} onClick={() => { onChange(""); close(); }}><span className="choice-picker-option-copy"><b>{t("Модель Codex по умолчанию")}</b><small>{t("Выбор текущей версии Codex")}</small></span><Check size={15} /></button>
          {visible.map((option) => <button key={option.value} type="button" className={`choice-picker-option ${value === option.value ? "selected" : ""}`} role="option" aria-selected={value === option.value} data-tooltip={[t(option.label), option.detail ? t(option.detail) : ""].filter(Boolean).join(" — ")} onClick={() => { onChange(option.value); close(); }}><span className="choice-picker-option-copy"><b>{t(option.label)}</b><small>{t(option.detail ?? "")}</small></span><Check size={15} /></button>)}
          {value.trim() && !modelOptions.some((option) => option.value === value.trim().toLowerCase()) && <button type="button" className="choice-picker-option selected" role="option" aria-selected="true" data-tooltip={`${t("Своя модель:")} ${value} — ${t("Использовать введённый идентификатор")}`} onClick={close}><span className="choice-picker-option-copy"><b>{t("Своя модель:")}{' '}{value}</b><small>{t("Использовать введённый идентификатор")}</small></span><Check size={15} /></button>}
        </motion.div>}
      </AnimatePresence>
    </div>
    <small className="model-picker-note">{t("Список популярных моделей обновлён по официальной документации. Доступность зависит от аккаунта и провайдера; свой идентификатор можно ввести вручную.")}</small>
  </div>;
}

function BackupPanel({ onExport, onImport, busy }: { onExport: (password: string) => void; onImport: (password: string) => void; busy: string }) {
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  return <div className="panel-content backup-content">
    <div className="modal-kicker"><HardDriveDownload size={14} /> {' '}{t("РЕЗЕРВНАЯ КОПИЯ")}</div><h2>{t("Аккаунты под рукой.")}</h2><p className="modal-intro">{t("Перенесите профили, общие настройки Codex и свою библиотеку prompts.")}</p>
    <div className="backup-preview"><div className="backup-orb"><FileKey2 size={23} /></div><div><b>{t("Полная копия")}</b><p>{t("OAuth‑аккаунты · config.toml · prompts · настройки приложения")}</p></div><span className="backup-format">.CODEXACCOUNTS</span></div>
    <label className="field-label password-field">{t("Пароль шифрования")}{' '}<span className="field-optional">{t("необязательно")}</span><div className="password-wrap"><input type={showPassword ? "text" : "password"} value={password} onChange={(event) => setPassword(event.target.value)} placeholder={t("Оставьте пустым, чтобы сохранить без пароля")} /><button aria-label={showPassword ? t("Скрыть пароль") : t("Показать пароль")} onClick={() => setShowPassword(!showPassword)}>{showPassword ? <EyeOff size={15} /> : <Eye size={15} />}</button></div><small>{password ? t("Пароль должен содержать не менее 8 символов.") : t("Экспорт без пароля содержит данные входа в открытом виде.")}</small></label>
    <div className="backup-actions"><button className="button-secondary" onClick={() => onImport(password)} disabled={!!busy}><Upload size={16} />{t("Импортировать")}</button><button className="button-primary" onClick={() => onExport(password)} disabled={!!busy}><ArrowDownToLine size={16} />{busy === "export" ? t("Подготовка…") : t("Экспортировать")}</button></div>
    {!password && <div className="backup-warning"><CircleHelp size={14} /><span>{t("Файл без пароля можно открыть. В нём будут данные входа аккаунтов.")}</span></div>}
  </div>;
}

function SettingRow({ icon, title, detail, children }: { icon: React.ReactNode; title: string; detail: string; children: React.ReactNode }) {
  return <div className="setting-row"><div className="setting-icon">{icon}</div><div className="setting-copy"><b>{title}</b><small>{t(detail)}</small></div>{children}</div>;
}

function Toggle({ checked, disabled, soundKey, labelOverride, onChange }: { checked: boolean; disabled?: boolean; soundKey?: string; labelOverride?: string; onChange: (checked: boolean) => void }) {
  const label = soundKey === "effects-toggle"
    ? (checked ? t("Выключить визуальные эффекты") : t("Включить визуальные эффекты"))
    : labelOverride ? t(labelOverride) : checked ? t("Выключить") : t("Включить");
  return <button type="button" role="switch" aria-checked={checked} aria-label={label} title={label} data-sound={soundKey} className={`toggle ${checked ? "toggle-on" : ""}`} disabled={disabled} onClick={() => onChange(!checked)}><span /></button>;
}

export default App;


