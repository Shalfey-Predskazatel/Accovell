#[cfg(windows)]
mod platform {
    use std::{collections::HashMap, f32::consts::TAU, sync::OnceLock};

    const SAMPLE_RATE: u32 = 22_050;
    const NOTES: &[(&str, &[u16])] = &[
        ("refresh", &[659, 784, 988]),
        ("backup", &[523, 659, 784]),
        ("settings", &[587, 740, 880]),
        ("codex", &[440, 554, 659]),
        ("sound_on", &[698, 880, 1047]),
        ("sound_off", &[880, 698, 554]),
        ("startup", &[523, 698, 880]),
        ("add", &[523, 659, 880]),
        ("switch", &[740, 587, 784]),
        ("success", &[587, 784, 988]),
        ("warning", &[659, 587, 494]),
        ("error", &[587, 494, 392]),
        ("delete", &[494, 440, 370]),
        ("minimize", &[740, 988]),
        ("hide", &[494, 370]),
        ("restore", &[392, 587, 784]),
        ("exit", &[392, 311, 262]),
        ("add_complete", &[523, 784, 1047]),
        ("switch_complete", &[740, 587, 880]),
        ("refresh_complete", &[988, 784, 1175]),
        ("settings_complete", &[587, 880, 698]),
        ("connect_complete", &[523, 880, 659]),
        ("disconnect_complete", &[440, 587, 494]),
        ("codex_complete", &[659, 880, 1047]),
        ("prompt_save_complete", &[494, 659, 880]),
        ("prompt_delete_complete", &[440, 392, 523]),
        ("settings_reset_complete", &[392, 523, 659]),
        ("app_reset_complete", &[659, 494, 392]),
        ("password_complete", &[1047, 880, 698]),
        ("unlock_complete", &[659, 988, 1175]),
        ("backup_export_complete", &[659, 1047, 880]),
        ("backup_import_complete", &[880, 659, 523]),
        ("exit_complete", &[784, 523, 392]),
        ("account_delete_complete", &[523, 494, 392]),
        ("auto_switch_complete", &[698, 880, 784, 988]),
        ("startup_toggle", &[587, 740]),
        ("tray_toggle", &[659, 880]),
        ("failover_toggle", &[523, 784]),
        ("prompt_add", &[392, 587, 698]),
        ("prompt_select", &[698, 784, 659]),
        ("prompt_remove_confirm", &[494, 587, 698]),
        ("exit_confirm", &[659, 523, 440]),
        ("settings_reset_confirm", &[440, 587, 698]),
        ("app_reset_confirm", &[392, 494, 659]),
        ("account_remove_confirm", &[494, 659, 523]),
        ("exit_prompt", &[784, 659]),
        ("settings_reset_prompt", &[587, 494, 440]),
        ("app_reset_prompt", &[494, 392]),
        ("account_remove_prompt", &[698, 523]),
        ("settings_confirm_step", &[523, 698]),
        ("app_reset_confirm_step", &[392, 523]),
        ("dialog_cancel", &[880, 659]),
        ("interval_minus_hour", &[494, 440]),
        ("interval_minus_minute", &[523, 440]),
        ("interval_minus_second", &[587, 494]),
        ("interval_plus_second", &[659, 784]),
        ("interval_plus_minute", &[740, 880]),
        ("interval_plus_hour", &[784, 1047]),
        ("account_add_error", &[784, 659, 523]),
        ("account_switch_error", &[988, 740, 587]),
        ("refresh_error", &[784, 659, 494, 392]),
        ("settings_error", &[784, 698, 494]),
        ("sound_setting_error", &[880, 740, 523]),
        ("connect_error", &[988, 784, 587, 523]),
        ("disconnect_error", &[880, 587, 494, 392]),
        ("codex_settings_error", &[880, 740, 523, 392]),
        ("prompt_save_error", &[784, 587, 494, 392]),
        ("prompt_delete_error", &[988, 698, 494, 392]),
        ("settings_reset_error", &[784, 523, 392, 494]),
        ("app_reset_error", &[988, 659, 392, 494]),
        ("password_error", &[880, 523, 392, 587]),
        ("unlock_error", &[988, 587, 392, 523]),
        ("backup_export_error", &[784, 659, 392, 587]),
        ("backup_import_error", &[988, 523, 392, 659]),
        ("exit_error", &[880, 784, 392, 587]),
        ("account_delete_error", &[784, 494, 392, 587]),
        ("dashboard_error", &[587, 659, 784, 659]),
        ("shutdown_error", &[659, 523, 392, 523]),
        ("proxy_error", &[740, 587, 392, 494]),
        ("access_error", &[659, 740, 784, 740]),
        ("interval_error", &[523, 587, 659, 587]),
        ("password_validation_error", &[740, 784, 880, 784]),
        ("prompt_name_error", &[587, 659, 740, 659]),
        ("tray_busy", &[880, 784, 659]),
        ("maximize", &[784, 988, 1175]),
    ];

    static SOUNDS: OnceLock<HashMap<&'static str, Box<[u8]>>> = OnceLock::new();

    pub fn play(name: &str, enabled: bool) {
        if !enabled {
            return;
        }
        let sounds = SOUNDS.get_or_init(|| {
            NOTES
                .iter()
                .map(|(name, notes)| {
                    let gain = match *name {
                        // Lifecycle sounds stay soft even at full Windows volume,
                        // and use separate levels so closing is the quietest cue.
                        "minimize" => 0.0035,
                        "hide" => 0.0028,
                        "exit" => 0.0022,
                        "restore" => 0.0034,
                        "startup" => 0.004,
                        _ => 0.026,
                    };
                    (*name, make_wave(notes, gain).into_boxed_slice())
                })
                .collect()
        });
        let Some(wave) = sounds.get(name) else {
            return;
        };
        const SND_MEMORY: u32 = 0x0004;
        const SND_ASYNC: u32 = 0x0001;
        const SND_NODEFAULT: u32 = 0x0002;
        #[link(name = "winmm")]
        unsafe extern "system" {
            fn PlaySoundW(sound: *const u16, module: *mut core::ffi::c_void, flags: u32) -> i32;
        }
        unsafe {
            PlaySoundW(
                wave.as_ptr().cast(),
                std::ptr::null_mut(),
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            );
        }
    }

    fn make_wave(notes: &[u16], gain: f32) -> Vec<u8> {
        let note_samples = (SAMPLE_RATE as usize * 68) / 1000;
        let gap_samples = (SAMPLE_RATE as usize * 12) / 1000;
        let mut samples = Vec::with_capacity(notes.len() * (note_samples + gap_samples));
        for note in notes {
            for sample in 0..note_samples {
                let time = sample as f32 / SAMPLE_RATE as f32;
                let progress = sample as f32 / note_samples as f32;
                let attack = (time / 0.014).clamp(0.0, 1.0);
                let release = ((1.0 - progress) / 0.34).clamp(0.0, 1.0);
                let envelope = attack * attack * release * release * (1.0 - 0.28 * progress);
                let fundamental = (TAU * f32::from(*note) * time).sin();
                let overtone = (TAU * f32::from(*note) * 2.0 * time).sin();
                let value = (fundamental + overtone * 0.105) * envelope * gain;
                samples.push((value * i16::MAX as f32) as i16);
            }
            samples.extend(std::iter::repeat_n(0, gap_samples));
        }

        let data_len = (samples.len() * 2) as u32;
        let mut wave = Vec::with_capacity(44 + data_len as usize);
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36 + data_len).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16_u32.to_le_bytes());
        wave.extend_from_slice(&1_u16.to_le_bytes());
        wave.extend_from_slice(&1_u16.to_le_bytes());
        wave.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        wave.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        wave.extend_from_slice(&2_u16.to_le_bytes());
        wave.extend_from_slice(&16_u16.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            wave.extend_from_slice(&sample.to_le_bytes());
        }
        wave
    }

    #[cfg(test)]
    mod tests {
        use super::NOTES;
        use std::collections::HashSet;

        #[test]
        fn every_named_action_uses_a_distinct_motif() {
            let mut motifs = HashSet::with_capacity(NOTES.len());
            for (name, notes) in NOTES {
                assert!(motifs.insert(notes.to_vec()), "duplicate motif for {name}");
            }
        }
    }
}

#[cfg(windows)]
pub use platform::play;

#[cfg(not(windows))]
pub fn play(_name: &str, _enabled: bool) {}
