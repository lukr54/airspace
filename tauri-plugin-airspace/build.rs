const COMMANDS: &[&str] = &[
    "create_host",
    "destroy_host",
    "set_holes",
    "clear_holes",
    "host_handle",
    "set_click_through",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
