// SPDX-License-Identifier: GPL-3.0-or-later
mod ui;
use gtk::{Application, prelude::*};
fn main() -> gtk::glib::ExitCode {
    let app = Application::builder()
        .application_id("org.calamares.NixOSRust")
        .build();
    app.connect_activate(ui::build);
    app.run()
}
