use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::i18n::tr;

/// Runs a step while its label line refreshes its own elapsed time once a second, so
/// a multi-minute dump or archive never looks like a hang.
pub(crate) fn run<T>(label: &str, work: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let stage = Stage::start(label);
    let result = work();
    stage.finish();
    result
}

/// A step that reports its own failures through the log and so cannot fail as a whole, but
/// still owes the console a ticking line.
pub(crate) fn step(label: &str, work: impl FnOnce()) {
    let _ = run(label, || {
        work();
        Ok(())
    });
}

struct Stage {
    label: String,
    started: Instant,
    stop: Arc<AtomicBool>,
    ticker: Option<thread::JoinHandle<()>>,
}

impl Stage {
    fn start(label: &str) -> Self {
        let started = Instant::now();
        let label = label.to_owned();
        print_line(&label, started.elapsed());
        let stop = Arc::new(AtomicBool::new(false));
        let ticker = {
            let (label, stop) = (label.clone(), Arc::clone(&stop));
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(1));
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    print_line(&label, started.elapsed());
                }
            })
        };
        Self {
            label,
            started,
            stop,
            ticker: Some(ticker),
        }
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
    }

    fn finish(mut self) {
        self.halt();
        println!("\r{}", line(&self.label, self.started.elapsed()));
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        self.halt();
    }
}

fn print_line(label: &str, elapsed: Duration) {
    // The trailing space covers a render shorter than the previous one.
    print!("\r{} ", line(label, elapsed));
    let _ = io::stdout().flush();
}

fn line(label: &str, elapsed: Duration) -> String {
    tr(
        "status.line",
        &[("label", &label), ("elapsed", &elapsed_text(elapsed))],
    )
}

fn elapsed_text(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{} min {} s", seconds / 60, seconds % 60)
}
