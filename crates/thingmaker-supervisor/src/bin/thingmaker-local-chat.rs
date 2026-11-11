//! A Local Models chat as its own program, for tests: the desktop runs the
//! same agent from its own executable (`--thingmaker-local-chat`).

fn main() {
    if let Err(error) = thingmaker_supervisor::agents::local::chat_agent::run() {
        eprintln!("thingmaker-local-chat: {error}");
        std::process::exit(1);
    }
}
