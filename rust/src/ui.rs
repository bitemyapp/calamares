// SPDX-License-Identifier: GPL-3.0-or-later
//! GTK owns widgets only. Filesystem discovery, validation, password hashing,
//! authorization and installation all run on workers with bounded messages.
use calamares_nixos::{
    Firmware, KEYBOARDS, LOCALES, Request, Settings,
    disk::{self, Disk},
    install::Event,
};
use gtk::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, DropDown, Entry, Label,
    Orientation, PasswordEntry, ProgressBar, Spinner, Stack, glib, prelude::*,
};
use std::{
    cell::{Cell, RefCell},
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    rc::Rc,
    sync::mpsc::{self, SyncSender},
    thread,
    time::Duration,
};
use zeroize::Zeroizing;

enum Message {
    Scanned(Result<(Vec<Disk>, Firmware), String>),
    Reviewed(Result<Box<Request>, String>),
    Event(Event),
    Finished(Result<(), String>),
}

fn launch(request: Request, send: SyncSender<Message>) -> anyhow::Result<()> {
    let helper = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("calamares-nixos-helper");
    let mut child = Command::new(option_env!("CALAMARES_PKEXEC").unwrap_or("pkexec"))
        .arg(helper)
        .arg("install")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let encoded = Zeroizing::new(serde_json::to_vec(&request)?);
    let input_result = child.stdin.take().unwrap().write_all(&encoded);
    drop(encoded);
    drop(request);
    // This is the GUI's worker, never its main thread. Do not abandon a helper
    // if the window's message receiver disappears.
    let mut completed = false;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line?;
        if let Ok(event) = serde_json::from_str(&line) {
            completed |= matches!(event, Event::Complete);
            let _ = send.send(Message::Event(event));
        }
    }
    let status = child.wait()?;
    input_result?;
    anyhow::ensure!(
        status.success(),
        "Installation did not finish (authorization canceled or helper failed, status {status}). See the status above; do not assume the disk is unchanged."
    );
    anyhow::ensure!(
        completed,
        "Helper exited without confirming installation completion"
    );
    Ok(())
}

fn label(text: &str) -> Label {
    let w = Label::new(Some(text));
    w.set_xalign(0.0);
    w.set_wrap(true);
    w
}
fn entry(text: &str) -> Entry {
    Entry::builder().text(text).hexpand(true).build()
}
fn row(grid: &gtk::Grid, index: i32, name: &str, widget: &impl IsA<gtk::Widget>) {
    grid.attach(&label(name), 0, index, 1, 1);
    grid.attach(widget, 1, index, 1, 1);
}
fn choice(dropdown: &DropDown, values: &[&str]) -> String {
    values
        .get(dropdown.selected() as usize)
        .unwrap_or(&"")
        .to_string()
}
fn disk_index(selected: u32, count: usize) -> Option<usize> {
    // GTK DropDown's selection model may reselect the first row when asked to
    // select INVALID_LIST_POSITION. A real placeholder row cannot select a disk.
    selected
        .checked_sub(1)
        .map(|n| n as usize)
        .filter(|n| *n < count)
}

pub fn build(app: &Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let window = ApplicationWindow::builder()
        .application(app)
        .title("NixOS · Rust Calamares")
        .default_width(900)
        .default_height(800)
        .build();
    let outer = GtkBox::new(Orientation::Vertical, 16);
    outer.set_margin_top(24);
    outer.set_margin_bottom(24);
    outer.set_margin_start(32);
    outer.set_margin_end(32);
    let title = label("Install NixOS with Determinate Nix");
    title.add_css_class("title-1");
    outer.append(&title);
    outer.append(&label(
        "Native Rust installer · Plasma desktop · pinned installation inputs",
    ));
    let stack = Stack::new();
    stack.set_vexpand(true);
    outer.append(&stack);
    let status_row = GtkBox::new(Orientation::Horizontal, 12);
    let spinner = Spinner::new();
    let status = label("Discovering disks…");
    status.set_hexpand(true);
    status.set_max_width_chars(95);
    status.set_lines(4);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.set_selectable(true);
    status_row.append(&spinner);
    status_row.append(&status);
    outer.append(&status_row);

    let setup = GtkBox::new(Orientation::Vertical, 14);
    let warning = label(
        "This release supports erasing one whole disk: GPT + ext4, EFI or legacy BIOS. It does not support manual partitioning, encryption, preserving another OS, or offline installation. Back up your data before continuing.",
    );
    warning.add_css_class("warning");
    setup.append(&warning);
    let grid = gtk::Grid::builder()
        .row_spacing(12)
        .column_spacing(18)
        .build();
    let disk_model = gtk::StringList::new(&["Select a disk — no device selected"]);
    let disks_menu = DropDown::new(Some(disk_model.clone()), None::<gtk::Expression>);
    disks_menu.set_selected(0);
    let scan = Button::with_label("Rescan disks");
    let disks_row = GtkBox::new(Orientation::Horizontal, 8);
    disks_menu.set_hexpand(true);
    disks_row.append(&disks_menu);
    disks_row.append(&scan);
    row(&grid, 0, "Disk to erase", &disks_row);
    let hostname = entry("nixos");
    hostname.set_max_length(63);
    row(&grid, 1, "Computer name", &hostname);
    let username = entry("");
    username.set_max_length(31);
    row(&grid, 2, "Username", &username);
    let full_name = entry("");
    full_name.set_max_length(128);
    row(&grid, 3, "Full name", &full_name);
    let password = PasswordEntry::builder().show_peek_icon(true).build();
    row(&grid, 4, "Password (12+ characters)", &password);
    let repeat = PasswordEntry::builder().show_peek_icon(true).build();
    row(&grid, 5, "Repeat password", &repeat);
    let locale = DropDown::from_strings(LOCALES);
    row(&grid, 6, "System locale", &locale);
    let timezone = entry("Etc/UTC");
    timezone.set_max_length(100);
    row(&grid, 7, "Time zone (e.g. America/Chicago)", &timezone);
    let keyboard = DropDown::from_strings(KEYBOARDS);
    row(&grid, 8, "Installed keyboard layout", &keyboard);
    setup.append(&grid);
    setup.append(&label("The live keyboard layout is unchanged. Passwords are entered using the current live layout; confirm it before proceeding."));
    let unfree =
        CheckButton::with_label("Allow unfree software (some hardware drivers require this)");
    setup.append(&unfree);
    let next = Button::with_label("Review installation");
    next.add_css_class("suggested-action");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&setup)
        .build();
    let setup_page = GtkBox::new(Orientation::Vertical, 12);
    setup_page.append(&scroll);
    setup_page.append(&next);
    stack.add_named(&setup_page, Some("setup"));

    let review = GtkBox::new(Orientation::Vertical, 18);
    let summary = label("");
    summary.set_selectable(true);
    review.append(&summary);
    let erase = entry("");
    erase.set_placeholder_text(Some("ERASE /dev/…"));
    review.append(&erase);
    let consent = CheckButton::with_label(
        "I understand that all partitions and data on this disk will be destroyed.",
    );
    review.append(&consent);
    let buttons = GtkBox::new(Orientation::Horizontal, 12);
    let back = Button::with_label("Back");
    let install = Button::with_label("Erase disk and install");
    install.add_css_class("destructive-action");
    install.set_sensitive(false);
    buttons.append(&back);
    buttons.append(&install);
    review.append(&buttons);
    stack.add_named(&review, Some("review"));

    let progress_page = GtkBox::new(Orientation::Vertical, 18);
    progress_page.append(&label("Installation in progress. Keep the computer connected to power and the network. Closing the installer is disabled until this operation finishes."));
    let progress = ProgressBar::new();
    progress.set_show_text(true);
    progress_page.append(&progress);
    let done = Button::with_label("Close installer");
    done.set_visible(false);
    progress_page.append(&done);
    stack.add_named(&progress_page, Some("progress"));
    stack.set_visible_child_name("setup");
    window.set_child(Some(&outer));

    let (send, receive) = mpsc::sync_channel::<Message>(64);
    let disks = Rc::new(RefCell::new(Vec::<Disk>::new()));
    let firmware = Rc::new(Cell::new(Firmware::Bios));
    let pending = Rc::new(RefCell::new(None::<Request>));
    let busy = Rc::new(Cell::new(false));
    let installing = Rc::new(Cell::new(false));
    let rescan: Rc<dyn Fn()> = Rc::new({
        let send = send.clone();
        let busy = busy.clone();
        let spinner = spinner.clone();
        let status = status.clone();
        let next = next.clone();
        let scan = scan.clone();
        move || {
            if busy.replace(true) {
                return;
            }
            spinner.start();
            status.set_text("Discovering disks…");
            next.set_sensitive(false);
            scan.set_sensitive(false);
            let send = send.clone();
            thread::spawn(move || {
                let result = disk::discover()
                    .map(|d| (d, Firmware::current()))
                    .map_err(|e| format!("{e:#}"));
                let _ = send.send(Message::Scanned(result));
            });
        }
    });
    scan.connect_clicked({
        let rescan = rescan.clone();
        move |_| rescan()
    });

    next.connect_clicked({
        let send = send.clone();
        let disks = disks.clone();
        let firmware = firmware.clone();
        let busy = busy.clone();
        let spinner = spinner.clone();
        let status = status.clone();
        let next = next.clone();
        let scan = scan.clone();
        let disks_menu = disks_menu.clone();
        let password = password.clone();
        let repeat = repeat.clone();
        let setup_page = setup_page.clone();
        move |_| {
            if busy.get() {
                return;
            }
            let selected = disk_index(disks_menu.selected(), disks.borrow().len());
            let Some(disk) = selected.and_then(|n| disks.borrow().get(n).cloned()) else {
                status.set_text("Select a disk first. No disk is selected automatically.");
                return;
            };
            if let Some(reason) = disk.blocked {
                status.set_text(&reason);
                return;
            }
            if password.text() != repeat.text() {
                status.set_text("The passwords do not match.");
                return;
            }
            let request = Request {
                confirmation: format!("ERASE {}", disk.identity.path),
                disk: disk.identity,
                firmware: firmware.get(),
                hostname: hostname.text().into(),
                username: username.text().into(),
                full_name: full_name.text().into(),
                password: password.text().into(),
                locale: choice(&locale, LOCALES),
                timezone: timezone.text().into(),
                keyboard: choice(&keyboard, KEYBOARDS),
                allow_unfree: unfree.is_active(),
            };
            busy.set(true);
            setup_page.set_sensitive(false);
            spinner.start();
            status.set_text("Checking your settings…");
            next.set_sensitive(false);
            scan.set_sensitive(false);
            let send = send.clone();
            thread::spawn(move || {
                let result = (|| -> anyhow::Result<Box<Request>> {
                    let settings = Settings::load()?;
                    calamares_nixos::validate(&request, &settings)?;
                    disk::revalidate(&request.disk)?;
                    Ok(Box::new(request))
                })()
                .map_err(|e| format!("{e:#}"));
                let _ = send.send(Message::Reviewed(result));
            });
        }
    });
    let confirm: Rc<dyn Fn()> = Rc::new({
        let erase = erase.clone();
        let consent = consent.clone();
        let pending = pending.clone();
        let install = install.clone();
        move || {
            install.set_sensitive(
                consent.is_active()
                    && pending
                        .borrow()
                        .as_ref()
                        .is_some_and(|r| erase.text() == format!("ERASE {}", r.disk.path)),
            );
        }
    });
    erase.connect_changed({
        let confirm = confirm.clone();
        move |_| confirm()
    });
    consent.connect_toggled(move |_| confirm());
    back.connect_clicked({
        let stack = stack.clone();
        let pending = pending.clone();
        let status = status.clone();
        move |_| {
            pending.borrow_mut().take();
            stack.set_visible_child_name("setup");
            status.set_text("Review your settings, then continue.");
        }
    });
    install.connect_clicked({
        let send=send.clone(); let pending=pending.clone(); let busy=busy.clone(); let installing=installing.clone(); let spinner=spinner.clone(); let stack=stack.clone(); let status=status.clone();
        let erase=erase.clone();
        move |_| {
            let Some(mut request)=pending.borrow_mut().take() else { return; };
            password.set_text(""); repeat.set_text("");
            request.confirmation=erase.text().into(); busy.set(true); installing.set(true); spinner.start(); stack.set_visible_child_name("progress"); status.set_text("Waiting for authorization. The helper will repeat all safety checks before any disk write.");
            let send=send.clone(); thread::spawn(move || { let outcome=launch(request,send.clone()).map_err(|e|format!("{e:#}")); let _=send.send(Message::Finished(outcome)); });
        }
    });
    done.connect_clicked({
        let window = window.clone();
        move |_| window.close()
    });
    window.connect_close_request({
        let installing = installing.clone();
        move |_| {
            if installing.get() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });

    glib::timeout_add_local(Duration::from_millis(80), {
        let weak = window.downgrade();
        let next = next.clone();
        move || {
            if weak.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            for message in receive.try_iter().take(32) {
                match message {
                    Message::Scanned(result) => {
                        busy.set(false);
                        spinner.stop();
                        scan.set_sensitive(true);
                        next.set_sensitive(true);
                        match result {
                            Ok((found, fw)) => {
                                disk_model.splice(0, disk_model.n_items(), &[]);
                                disk_model.append("Select a disk — no device selected");
                                for disk in &found {
                                    disk_model.append(&format!(
                                        "{} · {:.1} GiB · {}{}",
                                        disk.identity.path,
                                        disk.identity.bytes as f64 / 1024f64.powi(3),
                                        disk.identity.model,
                                        disk.blocked
                                            .as_ref()
                                            .map(|s| format!(" — unavailable: {s}"))
                                            .unwrap_or_default()
                                    ));
                                }
                                *disks.borrow_mut() = found;
                                disks_menu.set_selected(0);
                                firmware.set(fw);
                                status.set_text("Ready. Select a disk to erase; mounted devices are unavailable.");
                            }
                            Err(e) => status.set_text(&e),
                        }
                    }
                    Message::Reviewed(result) => {
                        setup_page.set_sensitive(true);
                        busy.set(false);
                        spinner.stop();
                        scan.set_sensitive(true);
                        next.set_sensitive(true);
                        match result {
                            Ok(r) => {
                                summary.set_text(&format!("ERASE ALL DATA ON {}\nModel: {}\nSerial: {}\nSize: {:.1} GiB\n\nInstall Plasma / {:?} / ext4\nHost: {} · User: {}\nLocale: {} · Time zone: {} · Keyboard: {}\nUnfree software: {}\n\nThe installation uses the media's pinned Determinate Nix flake. Root login is locked; your user can administer the system with sudo.\n\nType exactly: ERASE {}",r.disk.path,r.disk.model,r.disk.serial,r.disk.bytes as f64/1024f64.powi(3),r.firmware,r.hostname,r.username,r.locale,r.timezone,r.keyboard,r.allow_unfree,r.disk.path));
                                *pending.borrow_mut() = Some(*r);
                                erase.set_text("");
                                consent.set_active(false);
                                install.set_sensitive(false);
                                stack.set_visible_child_name("review");
                                status.set_text(
                                    "Nothing has been written. Confirm the device carefully.",
                                );
                            }
                            Err(e) => status.set_text(&e),
                        }
                    }
                    Message::Event(Event::Progress { step, message }) => {
                        progress.set_fraction(step as f64 / 6.0);
                        status.set_text(&message);
                    }
                    Message::Event(Event::Failed { message }) => {
                        // Bounded display work even if a command emits a large log.
                        let message: String = message.chars().take(6000).collect();
                        status.set_text(&format!("Installation failed: {message}"));
                    }
                    Message::Event(Event::Complete) => {
                        progress.set_fraction(1.0);
                    }
                    Message::Finished(result) => {
                        busy.set(false);
                        installing.set(false);
                        spinner.stop();
                        done.set_visible(true);
                        match result {Ok(())=>status.set_text("Installation complete. Shut down the live system, remove the media and boot the installed disk."),Err(e)=>{if !status.text().starts_with("Installation failed:"){status.set_text(&e);}}}
                    }
                }
            }
            glib::ControlFlow::Continue
        }
    });
    window.present();
    rescan();
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder_never_selects_a_disk() {
        assert_eq!(super::disk_index(0, 2), None);
        assert_eq!(super::disk_index(1, 2), Some(0));
        assert_eq!(super::disk_index(2, 2), Some(1));
        assert_eq!(super::disk_index(3, 2), None);
        assert_eq!(super::disk_index(u32::MAX, 2), None);
    }
}
