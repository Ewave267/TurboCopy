#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
compile_error!("TurboCopy Phase 1 supports Linux x86-64 and AArch64");

mod cli;
mod copy_engine;
mod fsutil;
mod scanner;
mod scheduler;

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
            if command.options.sync {
                eprintln!("tcopy: --sync compares size only; matching sizes do not guarantee matching contents");
            }
            let summary = match fsutil::install_signals().and_then(|_| scanner::plan(&command)) {
                Ok(roots) => scheduler::run(roots, command.options),
                Err(error) => {
                    let summary = std::sync::Arc::new(scheduler::Summary::default());
                    summary.fail(error);
                    summary
                }
            };
            summary.print(command.options.dry_run);
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
