//! LangCheck broker entry point (`langcheck.exe`).
//!
//! Bootstrap stage: by default this binary prints a banner and exits. The
//! `--spike` mode (delivery Step 01) wires the keyboard observer and the
//! UI-Automation focus-safety inspector together so the input/focus feasibility
//! measurements in ADR-0002 can be taken on a real desktop. The full coordinator,
//! correction loop, tray UI, and `--background` mode arrive in later steps (see
//! `blueprint.md` Section 24).
#![deny(unsafe_code)]

mod config;
mod coordinator;
mod diagnostics;
mod engine;
mod persistence;
mod tsf_broker;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use langcheck_core::{Boundary, IpcRequest, IpcResponse};
use langcheck_lexicon::compact_fst::CompactFstLexicon;
use langcheck_lexicon::PersonalDictionary;
use langcheck_windows::focus::{foreground_window_id, FieldClass, FocusInspector};
use langcheck_windows::input::{self, LowLevelKeyboardObserver};
use langcheck_windows::replace::{
    check_foreground_target, inject_text, ReplacementExecutor, ReplacementPlan, SendInputExecutor,
};
use langcheck_windows::startup::{self, SingleInstance};
use langcheck_windows::tray::{self, TrayHandler, TrayStatus};

use std::path::PathBuf;

use crate::config::{Config, CorrectionMode};
use crate::coordinator::{Coordinator, SharedState};
use crate::diagnostics::Metrics;

/// Per-user single-instance mutex name for the broker.
const INSTANCE_MUTEX: &str = "Local\\LangCheck-broker-singleton";

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--background") => run_background(),
        Some("--run") => run_autocorrect(),
        Some("--spike") => run_spike(),
        Some("--replace-demo") => run_replace_demo(),
        Some("--status") => show_status(),
        Some("--register-startup") => set_startup(true),
        Some("--unregister-startup") => set_startup(false),
        Some("--register-tsf") => set_tsf(true),
        Some("--unregister-tsf") => set_tsf(false),
        Some("--broker-serve") => run_broker_serve(),
        Some("--broker-eval") => run_broker_eval(std::env::args().nth(2)),
        Some("--tsf-selftest") => run_tsf_selftest(),
        Some("--reset") => reset_state(),
        _ => println!(
            "LangCheck {} (bootstrap build).\n\
             Modes:\n  \
             langcheck --background         run in the background with a tray icon (Step 08)\n  \
             langcheck --run                end-to-end autocorrect in the console (Step 06)\n  \
             langcheck --status             show enabled/mode/start-at-login state\n  \
             langcheck --register-startup   start LangCheck at sign-in (HKCU Run)\n  \
             langcheck --unregister-startup remove start-at-login\n  \
             langcheck --register-tsf       install the experimental TSF adapter (opt-in; UAC)\n  \
             langcheck --unregister-tsf     remove the TSF adapter\n  \
             langcheck --broker-serve       run only the IPC broker server (TSF adapter channel)\n  \
             langcheck --broker-eval WORD   ask the running broker about WORD (IPC client; diagnostic)\n  \
             langcheck --tsf-selftest       check the TSF adapter DLL can reach the broker over IPC\n  \
             langcheck --reset              delete all LangCheck state\n  \
             langcheck --spike              input/focus observer harness (ADR-0002)\n  \
             langcheck --replace-demo       SendInput replacement + integrity skip",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

/// A `--background` tray menu handler over the shared kill switch + config file.
struct BrokerTrayHandler {
    shared: Arc<SharedState>,
    config_path: Option<PathBuf>,
}

impl TrayHandler for BrokerTrayHandler {
    fn toggle_enabled(&self) {
        let next = !self.shared.enabled();
        self.shared.enabled.store(next, Ordering::SeqCst);
    }

    fn toggle_pause(&self) {
        let next = !self.shared.paused();
        self.shared.paused.store(next, Ordering::SeqCst);
    }

    fn open_settings(&self) {
        if let Some(path) = &self.config_path {
            tray::open_path(&path.to_string_lossy());
        }
    }

    fn request_exit(&self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
    }

    fn status(&self) -> TrayStatus {
        TrayStatus {
            enabled: self.shared.enabled(),
            paused: self.shared.paused(),
        }
    }
}

/// `--background`: the real broker — observer + focus thread + coordinator + tray
/// icon. Blocks on the tray message loop until the menu's Exit.
fn run_background() {
    let _instance = match SingleInstance::acquire(INSTANCE_MUTEX) {
        Some(instance) => instance,
        None => return, // already running; silent in background mode
    };
    let config = persistence::load_config();
    // Ensure config.toml exists so "Open settings" has a file to open.
    if persistence::config_path().is_some_and(|path| !path.exists()) {
        let _ = persistence::save_config(&config);
    }

    let lexicon = match CompactFstLexicon::production_en_us() {
        Ok(lexicon) => lexicon,
        Err(_) => return,
    };
    let shared = Arc::new(SharedState::new());
    let active = config.enabled && config.mode != CorrectionMode::Off;
    shared.enabled.store(active, Ordering::SeqCst);
    let metrics = Arc::new(Metrics::default());

    let (observer, focus_thread, coordinator_thread) =
        match start_engine(Box::new(lexicon), &config, &shared, &metrics) {
            Some(parts) => parts,
            None => return,
        };

    // TSF IPC broker: answer the precision adapter when it connects. Detached — it
    // idles until a same-user client appears and dies with the process on exit.
    spawn_tsf_broker(&shared);

    let handler = Box::new(BrokerTrayHandler {
        shared: Arc::clone(&shared),
        config_path: persistence::config_path(),
    });
    if let Err(_e) = tray::run_tray(handler) {
        // No tray (e.g. no shell): fall back to running until externally stopped.
        while !shared.is_shutdown() {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    shared.shutdown.store(true, Ordering::SeqCst);
    input::set_capture_allowed(false);
    observer.stop();
    let _ = coordinator_thread.join();
    let _ = focus_thread.join();
}

/// Spawn the TSF IPC broker on a detached thread, with its own lexicon and personal
/// dictionary (so it shares no mutable state with the coordinator). Best-effort: if
/// the lexicon can't load, the adapter simply finds no broker (fail open).
fn spawn_tsf_broker(shared: &Arc<SharedState>) {
    let Ok(lexicon) = CompactFstLexicon::production_en_us() else {
        return;
    };
    let personal = persistence::state_dir()
        .map(|dir| PersonalDictionary::load_dir(&dir))
        .unwrap_or_default();
    let shared = Arc::clone(shared);
    std::thread::spawn(move || tsf_broker::serve(shared, Box::new(lexicon), personal));
}

/// `--status`: print the persisted settings and start-at-login registration.
fn show_status() {
    let config = persistence::load_config();
    println!("LangCheck {}", env!("CARGO_PKG_VERSION"));
    println!("  enabled:        {}", config.enabled);
    println!("  mode:           {:?}", config.mode);
    println!("  language:       {}", config.language);
    println!(
        "  start at login: config={} registry={}",
        config.start_at_login,
        startup::is_start_at_login_enabled()
    );
    match persistence::config_path() {
        Some(path) => println!("  config file:    {}", path.display()),
        None => println!("  config file:    <LOCALAPPDATA unavailable>"),
    }
}

/// `--register-startup` / `--unregister-startup`: toggle start-at-login in both the
/// registry and the persisted config.
fn set_startup(enable: bool) {
    if let Err(e) = startup::set_start_at_login(enable) {
        eprintln!("failed to update start-at-login: {e}");
        return;
    }
    let mut config = persistence::load_config();
    config.start_at_login = enable;
    if let Err(e) = persistence::save_config(&config) {
        eprintln!("registry updated but failed to save config: {e}");
        return;
    }
    println!(
        "start-at-login {}.",
        if enable { "enabled" } else { "disabled" }
    );
}

/// `--register-tsf` / `--unregister-tsf`: install or remove the post-MVP TSF
/// precision adapter (opt-in; never automatic). TSF registration is machine-wide
/// and needs admin, so this self-elevates via UAC when not already elevated;
/// elevated, it loads `langcheck_tsf.dll` from beside this executable and calls
/// its in-process self-(un)registration entry point.
fn set_tsf(enable: bool) {
    let arg = if enable {
        "--register-tsf"
    } else {
        "--unregister-tsf"
    };
    // TSF text-service registration is machine-wide (HKLM) and requires admin, like
    // any IME. If not elevated, relaunch ourselves via UAC to do the work.
    if !langcheck_windows::tsf::is_elevated() {
        println!(
            "TSF adapter {} changes machine-wide input-method state and needs administrator",
            if enable { "registration" } else { "removal" }
        );
        println!("access (like any IME). Requesting elevation — accept the UAC prompt.");
        match langcheck_windows::tsf::relaunch_elevated(arg) {
            Ok(()) => println!("Elevated step launched in a new window."),
            Err(e) => eprintln!("elevation request failed: {e}"),
        }
        return;
    }
    let Some(dll) = tsf_dll_path() else {
        eprintln!("could not locate langcheck_tsf.dll next to the executable.");
        return;
    };
    if !dll.exists() {
        eprintln!("TSF adapter not found: {}", dll.display());
        eprintln!("(build the workspace so langcheck_tsf.dll sits beside langcheck.exe.)");
        return;
    }
    let result = if enable {
        langcheck_windows::tsf::register(&dll)
    } else {
        langcheck_windows::tsf::unregister(&dll)
    };
    match result {
        Ok(()) if enable => {
            println!("TSF adapter registered (machine-wide).");
            println!("NOTE: experimental and currently a NO-OP (no edit logic yet) — it adds an");
            println!("inert en-US input method. Remove it with `langcheck --unregister-tsf`.");
        }
        Ok(()) => println!("TSF adapter unregistered."),
        Err(e) => eprintln!(
            "failed to {} TSF adapter: {e}",
            if enable { "register" } else { "unregister" }
        ),
    }
}

/// Full path to the TSF adapter DLL that ships beside `langcheck.exe`.
fn tsf_dll_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("langcheck_tsf.dll"))
}

/// `--broker-serve`: run only the IPC broker server in the foreground (no keyboard
/// hook, no tray). Lets the TSF adapter channel be exercised on its own, and serves
/// as a headless mode. Blocks until the process is stopped.
fn run_broker_serve() {
    let lexicon = match CompactFstLexicon::production_en_us() {
        Ok(lexicon) => lexicon,
        Err(e) => {
            eprintln!("failed to load lexicon: {e}");
            return;
        }
    };
    let personal = persistence::state_dir()
        .map(|dir| PersonalDictionary::load_dir(&dir))
        .unwrap_or_default();
    let shared = Arc::new(SharedState::new());
    println!("LangCheck IPC broker serving on the per-user pipe. Press Ctrl-C to stop.");
    tsf_broker::serve(shared, Box::new(lexicon), personal);
}

/// `--broker-eval WORD`: act as an IPC client and print the broker's decision for
/// WORD (or send a Ping when no word is given). A diagnostic for the same-user IPC
/// channel the TSF adapter uses; requires a running `--broker-serve`/`--background`.
fn run_broker_eval(word: Option<String>) {
    let request = match word {
        Some(token) => IpcRequest::Evaluate {
            token,
            boundary: Boundary::Space,
        },
        None => IpcRequest::Ping,
    };
    match langcheck_ipc::request(&request) {
        Ok(IpcResponse::Pong) => println!("pong"),
        Ok(IpcResponse::Leave) => println!("leave (no correction)"),
        Ok(IpcResponse::Replace { replacement }) => println!("replace -> {replacement}"),
        Err(e) => eprintln!(
            "broker eval failed: {e}\n(is `langcheck --broker-serve` or `--background` running?)"
        ),
    }
}

/// `--tsf-selftest`: load the TSF adapter DLL and run its `LangCheckIpcSelfTest`
/// export, confirming the in-DLL IPC client can reach the broker. Requires a running
/// `--broker-serve`/`--background`. No elevation (only opens the pipe).
fn run_tsf_selftest() {
    let Some(dll) = tsf_dll_path() else {
        eprintln!("could not locate langcheck_tsf.dll next to the executable.");
        return;
    };
    if !dll.exists() {
        eprintln!("TSF adapter not found: {}", dll.display());
        return;
    }
    match langcheck_windows::tsf::ipc_selftest(&dll) {
        Ok(()) => println!("TSF adapter IPC self-test PASSED (reached the broker; wierd -> weird)."),
        Err(e) => eprintln!(
            "TSF adapter IPC self-test FAILED: {e}\n(is `langcheck --broker-serve` or `--background` running?)"
        ),
    }
}

/// `--reset`: delete all LangCheck state (config + user data).
fn reset_state() {
    match persistence::delete_all_state() {
        Ok(()) => println!("all LangCheck state deleted."),
        Err(e) => eprintln!("failed to delete state: {e}"),
    }
}

/// Step 06: the first working end-to-end autocorrect. Starts the observer, the
/// focus-safety thread (which gates capture), and the coordinator thread (which
/// corrects high-confidence typos), printing redacted metrics until Enter.
fn run_autocorrect() {
    // Enforce a single broker instance.
    let _instance = match SingleInstance::acquire(INSTANCE_MUTEX) {
        Some(instance) => instance,
        None => {
            eprintln!("LangCheck is already running.");
            return;
        }
    };
    let config = persistence::load_config();

    println!("LangCheck autocorrect (Step 06). Type in a normal text field — common typos are");
    println!("corrected after a space or period. Sensitive/unknown fields are skipped, and");
    println!("rapid typing cancels rather than mis-corrects. Press Enter to stop.\n");

    let lexicon = match CompactFstLexicon::production_en_us() {
        Ok(lexicon) => lexicon,
        Err(e) => {
            eprintln!("failed to load lexicon: {e}");
            return;
        }
    };
    let shared = Arc::new(SharedState::new());
    // Apply persisted settings: correction is active only if enabled and not Off.
    let active = config.enabled && config.mode != CorrectionMode::Off;
    shared.enabled.store(active, Ordering::SeqCst);
    let metrics = Arc::new(Metrics::default());

    let (observer, focus_thread, coordinator_thread) =
        match start_engine(Box::new(lexicon), &config, &shared, &metrics) {
            Some(parts) => parts,
            None => return,
        };

    // Stop on Enter.
    let stop_shared = Arc::clone(&shared);
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        stop_shared.shutdown.store(true, Ordering::SeqCst);
    });

    while !shared.is_shutdown() {
        std::thread::sleep(Duration::from_secs(2));
        let s = metrics.snapshot();
        println!(
            "events={} known={} auto={} applied={} undone={} cancelled={} [stale={} focus={} unsafe={} blocked={}] replace_fail={}",
            s.events_translated,
            s.known,
            s.autocorrected,
            s.corrections_applied,
            s.corrections_undone,
            s.commits_cancelled,
            s.cancel_stale,
            s.cancel_focus,
            s.cancel_unsafe,
            s.cancel_blocked,
            s.replace_failures,
        );
    }

    input::set_capture_allowed(false);
    observer.stop();
    let _ = coordinator_thread.join();
    let _ = focus_thread.join();
    let s = metrics.snapshot();
    println!(
        "\nstopped. corrections applied={}, cancelled={}",
        s.corrections_applied, s.commits_cancelled
    );
}

/// Start the observer plus the focus-safety and coordinator threads for `shared`,
/// returning them so the caller can join on shutdown. Shared by `--run` and
/// `--background`.
fn start_engine(
    lexicon: Box<dyn langcheck_lexicon::LexiconProvider>,
    config: &Config,
    shared: &Arc<SharedState>,
    metrics: &Arc<Metrics>,
) -> Option<(
    LowLevelKeyboardObserver,
    std::thread::JoinHandle<()>,
    std::thread::JoinHandle<()>,
)> {
    let personal = persistence::state_dir()
        .map(|dir| PersonalDictionary::load_dir(&dir))
        .unwrap_or_default();
    let undo_window = Duration::from_millis(config.undo_window_ms);
    let (tx, rx) = sync_channel(256);
    let observer = match LowLevelKeyboardObserver::start(tx) {
        Ok(observer) => observer,
        Err(e) => {
            eprintln!("failed to start keyboard observer: {e}");
            return None;
        }
    };

    // Focus-safety thread: classify the focused field, publish focus id, gate capture.
    let focus_shared = Arc::clone(shared);
    let focus_thread = std::thread::spawn(move || match FocusInspector::new() {
        Ok(inspector) => {
            while !focus_shared.is_shutdown() {
                let class = inspector.classify_focused();
                focus_shared
                    .focus_id
                    .store(foreground_window_id(), Ordering::SeqCst);
                // Fail closed: an unknown foreground process is treated as excluded.
                let process_excluded = langcheck_windows::policy::foreground_process_name()
                    .is_none_or(|name| langcheck_windows::policy::is_default_excluded(&name));
                let capture = class.capture_allowed()
                    && !process_excluded
                    && focus_shared.enabled()
                    && !focus_shared.paused();
                input::set_capture_allowed(capture);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Err(e) => eprintln!("focus inspector unavailable: {e}"),
    });

    // Coordinator thread: the correction loop.
    let coordinator_shared = Arc::clone(shared);
    let coordinator_metrics = Arc::clone(metrics);
    let coordinator_thread = std::thread::spawn(move || {
        let mut coordinator = Coordinator::new(
            lexicon,
            personal,
            undo_window,
            coordinator_shared,
            coordinator_metrics,
        );
        coordinator.run(&rx);
    });

    Some((observer, focus_thread, coordinator_thread))
}

/// Step 01 measurement harness: install the keyboard observer, run the focus
/// inspector on a dedicated COM thread (which gates `capture_allowed`), and print
/// **aggregate** counters only — never raw keystrokes or field contents
/// (`blueprint.md` Sections 12.1, 18.2).
fn run_spike() {
    println!(
        "LangCheck input/focus spike (ADR-0002). Aggregate stats only — no keystrokes logged."
    );
    println!("Type in different apps (Notepad, a browser, a PASSWORD field, a terminal) and watch");
    println!("`focus` and `captured`. Press Enter to stop.\n");

    let stop = Arc::new(AtomicBool::new(false));
    let focus_code = Arc::new(AtomicU8::new(class_code(FieldClass::Unknown)));

    let (tx, rx) = sync_channel(256);
    let observer = match LowLevelKeyboardObserver::start(tx) {
        Ok(observer) => observer,
        Err(e) => {
            eprintln!("failed to start keyboard observer: {e}");
            return;
        }
    };

    // Drain the channel so it never backs up (drops are then a true signal).
    let drain_stop = Arc::clone(&stop);
    let drainer = std::thread::spawn(move || loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(_event) => {}
            Err(RecvTimeoutError::Timeout) => {
                if drain_stop.load(Ordering::SeqCst) {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    });

    // Focus-safety thread: classify the focused field and toggle capture.
    let focus_stop = Arc::clone(&stop);
    let focus_shared = Arc::clone(&focus_code);
    let focus_thread = std::thread::spawn(move || match FocusInspector::new() {
        Ok(inspector) => {
            while !focus_stop.load(Ordering::SeqCst) {
                let class = inspector.classify_focused();
                input::set_capture_allowed(class.capture_allowed());
                focus_shared.store(class_code(class), Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Err(e) => eprintln!("focus inspector unavailable: {e}"),
    });

    // Stop on Enter.
    let input_stop = Arc::clone(&stop);
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        input_stop.store(true, Ordering::SeqCst);
    });

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(1));
        println!(
            "captured={:>6}  dropped={:>4}  capture_allowed={:<5}  focus={}",
            input::generation(),
            input::dropped_count(),
            input::capture_allowed(),
            class_name(focus_code.load(Ordering::SeqCst)),
        );
    }

    observer.stop();
    let _ = focus_thread.join();
    let _ = drainer.join();
    println!(
        "\nspike stopped: captured={}, dropped={}",
        input::generation(),
        input::dropped_count()
    );
}

/// Step 05 manual-verification harness: type "teh " into the focused field and
/// correct it to "the " via the executor. On a higher-integrity (elevated) target
/// it reports the integrity skip instead. (Password-field skipping is the focus
/// inspector's job — verify that with `--spike`.)
fn run_replace_demo() {
    println!("Replacement demo (Step 05). Focus a text field; LangCheck will type \"teh \"");
    println!("and correct it to \"the \". Focus an ELEVATED window to see the integrity skip.");
    for remaining in (1..=3).rev() {
        println!("  starting in {remaining}...");
        std::thread::sleep(Duration::from_secs(1));
    }

    if let Err(e) = check_foreground_target() {
        println!("skipped (no replacement performed): {e}");
        return;
    }
    if let Err(e) = inject_text("teh ") {
        println!("could not type demo text: {e}");
        return;
    }
    std::thread::sleep(Duration::from_millis(300));

    let plan = ReplacementPlan {
        focus_id: 0,
        expected_generation: 0,
        original: "teh".to_owned(),
        replacement: "the".to_owned(),
        boundary: Boundary::Space,
    };
    let mut executor = SendInputExecutor;
    match executor.execute(&plan) {
        Ok(undo) => println!("replaced {:?} -> {:?}", undo.original, undo.replacement),
        Err(e) => println!("replacement skipped/failed: {e}"),
    }
}

fn class_code(class: FieldClass) -> u8 {
    match class {
        FieldClass::NormalProse => 0,
        FieldClass::Sensitive => 1,
        FieldClass::NonProse => 2,
        FieldClass::Unknown => 3,
    }
}

fn class_name(code: u8) -> &'static str {
    match code {
        0 => "NormalProse",
        1 => "Sensitive",
        2 => "NonProse",
        _ => "Unknown",
    }
}
