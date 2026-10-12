#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
compile_error!("TurboCopy supports Linux x86-64 and AArch64");

mod cli;
mod copy_engine;
mod fsutil;
mod hardware;
mod progress;
mod scanner;
mod scheduler;
mod storage_detect;
mod tuner;

fn main() {
    let code = match cli::parse(std::env::args_os().skip(1)) {
        Ok(cli::Action::Help) => {
            println!("{}", cli::HELP);
            0
        }
        Ok(cli::Action::Version) => {
            println!("TurboCopy {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Ok(cli::Action::Copy(command)) => {
            let start = std::time::Instant::now();
            if command.options.sync {
                eprintln!("tcopy: --sync compares size only; matching sizes do not guarantee matching contents");
            }
            let summary = match fsutil::install_signals().and_then(|_| scanner::plan(&command)) {
                Ok(roots) => scheduler::run(roots, command.options, start),
                Err(error) => {
                    let summary = std::sync::Arc::new(scheduler::Summary::new(
                        command.options,
                        Vec::new(),
                        start,
                    ));
                    summary.fail(error);
                    summary
                }
            };
            summary.print();
            let signal = fsutil::received_signal();
            if signal != 0 {
                128 + signal
            } else if summary.failures.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                1
            } else {
                0
            }
        }
        Err(error) => {
            eprintln!("tcopy: {error}");
            2
        }
    };
    std::process::exit(code);
}
