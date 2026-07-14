mod commands;
mod filesystem;
mod local_agent;
mod secure_store;
mod system;
mod updater;

fn main() {
    println!(
        "wt-media-desktop scaffold ready with {} local agent commands",
        local_agent::LocalAgentBridge::command_names().len()
    );
}
