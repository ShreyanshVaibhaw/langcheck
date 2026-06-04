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
mod persistence;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use langcheck_core::Boundary;
use langcheck_windows::focus::{FieldClass, FocusInspector};
use langcheck_windows::input::{self, LowLevelKeyboardObserver};
use langcheck_windows::replace::{
    check_foreground_target, inject_text, ReplacementExecutor, ReplacementPlan, SendInputExecutor,
};

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--spike") => run_spike(),
        Some("--replace-demo") => run_replace_demo(),
        _ => println!(
            "LangCheck {} (bootstrap build) — no correction functionality yet.\n\
             Harnesses for manual verification:\n  \
             langcheck --spike          input/focus observer (Step 01, ADR-0002)\n  \
             langcheck --replace-demo   SendInput replacement + integrity skip (Step 05)",
            env!("CARGO_PKG_VERSION")
        ),
    }
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
