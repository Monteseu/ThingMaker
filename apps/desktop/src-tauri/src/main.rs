// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // An agent starting its ThingMaker MCP server runs this executable as a
    // stdio relay; no window, no app state.
    if std::env::args().nth(1).as_deref() == Some(thingmaker_supervisor::delegation::socket::RELAY_ARG) {
        if let Err(error) = thingmaker_supervisor::delegation::socket::relay() {
            eprintln!("thingmaker-mcp: {error}");
            std::process::exit(1);
        }
        return;
    }
    // A Local Models chat runs this executable as its ACP agent: it passes
    // messages to Ollama on this computer and has no tools.
    if std::env::args().nth(1).as_deref() == Some(thingmaker_supervisor::agents::local::chat_agent::CHAT_ARG) {
        if let Err(error) = thingmaker_supervisor::agents::local::chat_agent::run() {
            eprintln!("thingmaker-local-chat: {error}");
            std::process::exit(1);
        }
        return;
    }
    // A Local Models agent on Windows is started through this executable,
    // which puts it in the AppContainer and waits for it.
    if std::env::args().nth(1).as_deref() == Some(thingmaker_appcontainer::LAUNCH_ARG) {
        let mut rest = std::env::args().skip(2);
        let Some(program) = rest.next() else {
            eprintln!("thingmaker-appcontainer: no program given");
            std::process::exit(2);
        };
        let args: Vec<String> = rest.collect();
        match thingmaker_appcontainer::run_inside(&program, &args) {
            Ok(code) => std::process::exit(code as i32),
            Err(error) => {
                eprintln!("thingmaker-appcontainer: {error}");
                std::process::exit(1);
            }
        }
    }
    thingmaker_desktop_lib::run()
}
