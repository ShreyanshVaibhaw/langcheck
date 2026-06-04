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

use langcheck_windows::focus::{FieldClass, FocusInspector};
use langcheck_windows::input::{self, LowLevelKeyboardObserver};

fn main() {
    if std::env::args().skip(1).any(|a| a == "--spike") {
        run_spike();
    } else {
        println!(
            "LangCheck {} (bootstrap build) — no correction functionality yet. \
             Run `langcheck --spike` for the Step 01 input/focus measurement harness.",
            env!("CARGO_PKG_VERSION")
        );
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
