use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    time::Duration,
};
use veyra_engine::fixture::{self, Matcher, Record};

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(3);
        }
    }
}
fn run() -> io::Result<i32> {
    let path = PathBuf::from(
        std::env::var_os("VEYRA_FAKE_FIXTURE")
            .ok_or_else(|| io::Error::other("VEYRA_FAKE_FIXTURE is missing"))?,
    );
    let mut matcher = Matcher::default();
    matcher.bind("<cwd>", serde_json::json!(std::env::current_dir()?));
    let args: Vec<_> = std::env::args().collect();
    if let Some(index) = args
        .iter()
        .position(|a| a == "--session-id" || a == "--resume")
        && let Some(native) = args.get(index + 1)
    {
        matcher.bind("<id:session>", serde_json::json!(native));
    }
    let mut input = io::stdin().lock().lines();
    let mut output = io::stdout().lock();
    for record in fixture::load(&path)? {
        match record {
            Record::TranscriptStart { .. } => {}
            Record::ExpectOutbound { line } => {
                let actual = input
                    .next()
                    .transpose()?
                    .ok_or_else(|| io::Error::other(format!("expected {line}, actual EOF")))?;
                let value = serde_json::from_str(&actual)?;
                if !matcher.matches(&line, &value) {
                    return Err(io::Error::other(format!(
                        "expected {line}, actual {actual}"
                    )));
                }
            }
            Record::EmitInbound { line, after_ms } => {
                if let Some(delay) = after_ms {
                    std::thread::sleep(Duration::from_millis(delay));
                }
                writeln!(output, "{}", matcher.expand(line))?;
                output.flush()?;
            }
            Record::Exit { code, stderr } => {
                for line in stderr {
                    eprintln!("{line}");
                }
                return Ok(code);
            }
        }
    }
    Ok(0)
}
