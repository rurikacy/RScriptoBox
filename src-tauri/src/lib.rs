mod commands;
mod library;
mod models;
mod process_windows;
mod runner;
mod runtimes;
mod storage;

pub fn run() {
    commands::launch();
}
