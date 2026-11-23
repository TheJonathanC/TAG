use std::io;

fn main() {
    let mut script_path: Option<String> = None;
    let mut prompt: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--script" {
            script_path = args.next();
        } else if let Some(p) = a.strip_prefix("--script=") {
            script_path = Some(p.to_string());
        } else if !a.starts_with('-') {
            prompt = Some(a);
        }
    }
    let script_path = script_path.or_else(|| std::env::var("FAKE_AGENT_SCRIPT").ok());
    let script = match script_path {
        Some(p) => match std::fs::read_to_string(&p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fake-agent: cannot read script {p}: {e}");
                std::process::exit(2);
            }
        },
        None => std::env::var("FAKE_AGENT_SCRIPT_TEXT").unwrap_or_default(),
    };
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let cfg = fake_agent::Config::from_env();
    let code = fake_agent::run(&script, prompt.as_deref(), &cfg, &mut stdin, &mut out);
    std::process::exit(code);
}
