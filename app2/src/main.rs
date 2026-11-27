#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process;

fn main() {
    mediadeka::application().run(|cx| match mediadeka::open_main_window(cx) {
        Ok(_) => cx.activate(true),
        Err(error) => {
            eprintln!("Не удалось открыть окно Mediadeka: {error:#}");
            process::exit(1);
        }
    });
}
