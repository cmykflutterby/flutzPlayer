use std::{fs, time::Duration};

use flutz_app::repaint_policy::{
    meter_decay_factor, repaint_interval, RepaintInputs, ACTIVE_REPAINT_INTERVAL,
    BACKGROUND_WORK_REPAINT_INTERVAL, IDLE_REPAINT_INTERVAL, PLAYING_REPAINT_INTERVAL,
};

type ProbeResult = Result<(), Box<dyn std::error::Error>>;

fn main() -> ProbeResult {
    check_policy_table()?;
    check_meter_decay()?;
    check_source_wiring()?;
    println!(
        "scenario=repaint-cadence policy=ok meter_decay=ok ui_waker_wiring=ok media_waker_wiring=ok status=ok"
    );
    Ok(())
}

fn check_policy_table() -> ProbeResult {
    let playing = RepaintInputs {
        playing: true,
        seconds_until_track_end: Some(30.0),
        ..RepaintInputs::default()
    };
    let cases: [(&str, RepaintInputs, Duration); 12] = [
        ("idle", RepaintInputs::default(), IDLE_REPAINT_INTERVAL),
        ("playing", playing, PLAYING_REPAINT_INTERVAL),
        (
            "playing-unknown-duration",
            RepaintInputs {
                seconds_until_track_end: None,
                ..playing
            },
            PLAYING_REPAINT_INTERVAL,
        ),
        (
            "playing-near-end",
            RepaintInputs {
                seconds_until_track_end: Some(0.75),
                ..playing
            },
            ACTIVE_REPAINT_INTERVAL,
        ),
        (
            "playing-at-window-edge",
            RepaintInputs {
                seconds_until_track_end: Some(1.0),
                ..playing
            },
            ACTIVE_REPAINT_INTERVAL,
        ),
        (
            "playing-past-end",
            RepaintInputs {
                seconds_until_track_end: Some(-0.2),
                ..playing
            },
            ACTIVE_REPAINT_INTERVAL,
        ),
        (
            "playing-minimized",
            RepaintInputs {
                minimized: true,
                ..playing
            },
            IDLE_REPAINT_INTERVAL,
        ),
        (
            "playing-minimized-near-end",
            RepaintInputs {
                minimized: true,
                seconds_until_track_end: Some(0.5),
                ..playing
            },
            ACTIVE_REPAINT_INTERVAL,
        ),
        (
            "playing-minimized-background-work",
            RepaintInputs {
                minimized: true,
                background_work_pending: true,
                ..playing
            },
            BACKGROUND_WORK_REPAINT_INTERVAL,
        ),
        (
            "paused-near-end-not-fast",
            RepaintInputs {
                seconds_until_track_end: Some(0.5),
                ..RepaintInputs::default()
            },
            IDLE_REPAINT_INTERVAL,
        ),
        (
            "idle-background-work",
            RepaintInputs {
                background_work_pending: true,
                ..RepaintInputs::default()
            },
            BACKGROUND_WORK_REPAINT_INTERVAL,
        ),
        (
            "tracing-overrides-idle",
            RepaintInputs {
                frame_tracing_enabled: true,
                ..RepaintInputs::default()
            },
            ACTIVE_REPAINT_INTERVAL,
        ),
    ];
    for (name, inputs, expected) in cases {
        let actual = repaint_interval(inputs);
        if actual != expected {
            return Err(
                format!("policy case {name}: expected {expected:?}, got {actual:?}").into(),
            );
        }
        println!("policy.{name}={}ms", actual.as_millis());
    }
    if ACTIVE_REPAINT_INTERVAL != Duration::from_millis(16) {
        return Err("active interval no longer matches the historical 16 ms".into());
    }
    Ok(())
}

fn check_meter_decay() -> ProbeResult {
    let at_60hz = meter_decay_factor(1.0 / 60.0);
    if (at_60hz - 0.82).abs() > 1e-5 {
        return Err(format!("60 fps decay {at_60hz} != historical 0.82").into());
    }
    let one_30hz_frame = meter_decay_factor(1.0 / 30.0);
    let two_60hz_frames = at_60hz * at_60hz;
    if (one_30hz_frame - two_60hz_frames).abs() > 1e-5 {
        return Err(format!(
            "30 fps decay {one_30hz_frame} != two 60 fps frames {two_60hz_frames}"
        )
        .into());
    }
    let one_second: f32 = (0..60).map(|_| at_60hz).product();
    let at_4hz: f32 = (0..4).map(|_| meter_decay_factor(0.25)).product();
    if (one_second - at_4hz).abs() > 1e-4 {
        return Err(
            format!("1 s decay differs by cadence: 60 fps {one_second}, 4 fps {at_4hz}").into(),
        );
    }
    for (label, elapsed) in [("zero", 0.0), ("negative", -1.0), ("nan", f32::NAN)] {
        let factor = meter_decay_factor(elapsed);
        if factor != 1.0 {
            return Err(format!("{label} elapsed decay should be 1.0, got {factor}").into());
        }
    }
    let clamped = meter_decay_factor(1_000.0);
    if (clamped - meter_decay_factor(1.0)).abs() > f32::EPSILON
        || clamped.is_nan()
        || clamped <= 0.0
    {
        return Err(format!("large elapsed decay not clamped to 1 s: {clamped}").into());
    }
    println!("meter_decay.60hz={at_60hz:.5} meter_decay.30hz={one_30hz_frame:.5} meter_decay.1s={one_second:.6}");
    Ok(())
}

fn check_source_wiring() -> ProbeResult {
    let lifecycle = fs::read_to_string("crates/flutz_app/src/app/lifecycle.rs")?;
    let update = function_body(&lifecycle, "fn update(")?;
    require(
        !update.contains("from_millis(16)"),
        "update() still requests a fixed 16 ms repaint",
    )?;
    require(
        update.contains("self.schedule_next_repaint(context);"),
        "update() does not call schedule_next_repaint",
    )?;
    require(
        update.contains("self.refresh_realtime_feedback(frame_seconds);"),
        "update() does not pass frame time to refresh_realtime_feedback",
    )?;
    let new_fn = function_body(&lifecycle, "fn new(")?;
    let waker_at = new_fn
        .find("app.playback.set_ui_waker(")
        .ok_or("new() does not install the playback UI waker")?;
    let media_waker_at = new_fn
        .find("app.media_controls.set_command_waker(")
        .ok_or("new() does not install the media command waker")?;
    let launch_at = new_fn
        .find("config.launch_open_path")
        .ok_or("new() launch path handling not found")?;
    require(
        waker_at < launch_at && media_waker_at < launch_at,
        "wakers must be installed before the launch path can open an audio stream",
    )?;

    let diagnostics = fs::read_to_string("crates/flutz_app/src/app/diagnostics.rs")?;
    let feedback = function_body(&diagnostics, "fn refresh_realtime_feedback(")?;
    require(
        !feedback.contains("*= 0.82"),
        "meter decay is still per-frame",
    )?;
    require(
        feedback.contains("meter_decay_factor("),
        "meter decay does not use meter_decay_factor",
    )?;

    let controller = fs::read_to_string("crates/flutz_app/src/playback/controller.rs")?;
    let stream = function_body(&controller, "fn ensure_audio_stream(")?;
    require(
        stream.matches("wake_ui_on_playback_end(").count() == 2,
        "both decoded and MIDI audio callbacks must call wake_ui_on_playback_end",
    )?;
    require(
        stream
            .matches("let audio_ui_waker = self.ui_waker.clone();")
            .count()
            == 2,
        "both audio callbacks must capture the UI waker",
    )?;
    let midi_tail = &stream[stream
        .rfind("let playing_now = engine.state()")
        .ok_or("MIDI callback playing_now missing")?..];
    let drop_engine_at = midi_tail
        .find("drop(engine);")
        .ok_or("MIDI callback does not release the engine lock")?;
    let wake_at = midi_tail
        .find("wake_ui_on_playback_end(")
        .ok_or("MIDI callback wake missing")?;
    require(
        drop_engine_at < wake_at,
        "MIDI callback must release locks before waking the UI",
    )?;
    let edge = function_body(&controller, "fn wake_ui_on_playback_end(")?;
    require(
        edge.contains("*was_playing && !playing_now"),
        "UI wake must fire only on the playing -> stopped edge",
    )?;

    let queue = fs::read_to_string("crates/flutz_media_controls/src/windows/mod.rs")?;
    let push = function_body(&queue, "fn push(")?;
    let unlock_at = push
        .find("drop(commands);")
        .ok_or("command push does not release the queue lock")?;
    let notify_at = push
        .find("waker();")
        .ok_or("command push does not call the waker")?;
    require(
        unlock_at < notify_at,
        "command waker must run after the queue lock is released",
    )?;
    let smtc = fs::read_to_string("crates/flutz_media_controls/src/windows/smtc.rs")?;
    let relay = fs::read_to_string("crates/flutz_media_controls/src/windows/relay.rs")?;
    require(
        !smtc.contains("push_back("),
        "SMTC handlers bypass CommandQueue::push",
    )?;
    require(
        !relay.contains("push_back("),
        "relay bypasses CommandQueue::push",
    )?;
    require(
        smtc.contains("fn set_command_waker(") && smtc.contains("self.commands.set_waker(waker)"),
        "Windows backend does not forward set_command_waker",
    )?;

    let policy = fs::read_to_string("crates/flutz_app/src/repaint_policy.rs")?;
    require(
        !policy.contains("#[test]") && !policy.contains("cfg(test)"),
        "inline tests are not allowed",
    )?;
    Ok(())
}

fn require(condition: bool, message: &str) -> ProbeResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn function_body<'a>(
    source: &'a str,
    signature: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    let signature_start = source
        .find(signature)
        .ok_or_else(|| format!("missing function signature: {signature}"))?;
    let body_start = source[signature_start..]
        .find('{')
        .map(|offset| signature_start + offset)
        .ok_or_else(|| format!("missing function body: {signature}"))?;
    let mut depth = 0usize;
    for (offset, byte) in source[body_start..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(&source[body_start..=body_start + offset]);
                }
            }
            _ => {}
        }
    }
    Err(format!("unterminated function body: {signature}").into())
}
