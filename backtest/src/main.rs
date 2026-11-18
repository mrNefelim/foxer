use std::path::Path;

fn main() {
    if let Err(error) = execute() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn execute() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = |index: usize| Path::new(&args[index]);
    match args.first().map(String::as_str) {
        Some("--help") if args.len() == 1 => {
            println!("forex-backtest swing-history HISTORY_ROOT SYMBOL SWING_CONFIG.json NEW_TRADES.csv\nforex-backtest swing-replay MT5_M1.csv SWING_CONFIG.json NEW_TRADES.csv [INDEX_M1.csv INDEX_CONFIG.json]\nforex-backtest dxy-build EURUSD.csv USDJPY.csv GBPUSD.csv USDCAD.csv USDSEK.csv USDCHF.csv NEW_INDEX.csv\nforex-backtest --report-only SWING_RESULT_DIR");
            Ok(())
        }
        Some("--report-only") if args.len() == 2 => forex_backtest::report(path(1)),
        Some("swing-replay") if args.len() == 4 || args.len() == 6 => {
            let index = (args.len() == 6).then(|| (path(4), path(5)));
            forex_backtest::swing_replay::run(path(1), path(2), path(3), index)
        }
        Some("swing-history") if args.len() == 5 => {
            forex_backtest::swing_history::run(path(1), &args[2], path(3), path(4))
        }
        Some("dxy-build") if args.len() == 8 => {
            let inputs: Vec<&Path> = args[1..7].iter().map(Path::new).collect();
            forex_backtest::swing_replay::build(&inputs, path(7))
        }
        Some("--report-only" | "swing-replay" | "swing-history" | "dxy-build" | "--help") => {
            Err("invalid arguments; use --help for Swing commands".into())
        }
        Some(command) => Err(format!(
            "unknown command: {command}; use --help for Swing commands"
        )),
        None => Err("a Swing command is required; use --help".into()),
    }
}
