// SPDX-License-Identifier: GPL-3.0-or-later
//! GTK owns widgets only. Filesystem discovery, validation, password hashing,
//! authorization and installation all run on workers with bounded messages.
use calamares_nixos::{
    ConfirmedInstall, Desktop, Filesystem, Firmware, InstallPlan, KEYBOARDS, LOCALES, RawRequest,
    Settings, applications,
    disk::{self, Disk},
    install::Event,
    timezone,
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
    Reviewed(Result<Box<InstallPlan>, String>),
    Event(Event),
    Finished(Result<(), String>),
    Zone(u64, Result<timezone::Detection, String>),
}

fn launch(confirmed: ConfirmedInstall, send: SyncSender<Message>) -> anyhow::Result<()> {
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
    let request = confirmed.into_request();
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

fn application_page(unfree: &CheckButton) -> (GtkBox, Vec<CheckButton>) {
    let page = GtkBox::new(Orientation::Vertical, 12);
    page.append(&label("Choose applications to have ready after installation. You can select any combination and sign in to your accounts later."));
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search applications")
        .build();
    page.append(&search);
    let count = label("");
    page.append(&count);
    let checks: Vec<_> = applications::catalog()
        .iter()
        .map(|app| {
            let check = CheckButton::with_label(&app.name);
            check.set_active(app.id == "firefox");
            check
        })
        .collect();
    let mut groups = Vec::new();
    let mut previous = "";
    let mut group = GtkBox::new(Orientation::Vertical, 10);
    let mut rows = Vec::new();
    for (app, check) in applications::catalog().iter().zip(&checks) {
        if app.category != previous {
            if !rows.is_empty() {
                groups.push((group, std::mem::take(&mut rows)));
            }
            group = GtkBox::new(Orientation::Vertical, 10);
            let heading = label(&app.category);
            heading.add_css_class("heading");
            group.append(&heading);
            page.append(&group);
            previous = &app.category;
        }
        let item = GtkBox::new(Orientation::Vertical, 3);
        item.append(check);
        let description = label(&app.description);
        description.set_margin_start(28);
        item.append(&description);
        let badges = [
            app.terminal.as_ref().map(|_| "Terminal application"),
            app.unfree.then_some("Proprietary software"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        if !badges.is_empty() {
            let badges = label(&badges);
            badges.add_css_class("dim-label");
            badges.set_margin_start(28);
            item.append(&badges);
        }
        group.append(&item);
        rows.push((
            item,
            format!("{} {} {}", app.name, app.description, app.category).to_lowercase(),
        ));
    }
    groups.push((group, rows));
    search.connect_search_changed(move |search| {
        let query = search.text().to_lowercase();
        for (group, rows) in &groups {
            let mut visible = false;
            for (row, text) in rows {
                let matches = text.contains(query.trim());
                row.set_visible(matches);
                visible |= matches;
            }
            group.set_visible(visible);
        }
    });
    let updating = Rc::new(Cell::new(false));
    let refresh: Rc<dyn Fn()> = Rc::new({
        let checks: Vec<_> = checks.iter().map(CheckButton::downgrade).collect();
        let unfree = unfree.downgrade();
        let count = count.downgrade();
        move || {
            if updating.replace(true) {
                return;
            }
            if let (Some(unfree), Some(count)) = (unfree.upgrade(), count.upgrade()) {
                let checks: Vec<_> = checks.iter().filter_map(glib::WeakRef::upgrade).collect();
                if checks.len() == applications::catalog().len() {
                    for (app, check) in applications::catalog().iter().zip(&checks) {
                        if app.unfree && !unfree.is_active() {
                            check.set_active(false);
                        }
                    }
                    let required: std::collections::BTreeSet<_> = applications::catalog()
                        .iter()
                        .zip(&checks)
                        .filter(|(_, check)| check.is_active())
                        .flat_map(|(app, _)| app.requires.iter())
                        .collect();
                    for (app, check) in applications::catalog().iter().zip(&checks) {
                        let needed = required.contains(&app.id);
                        if needed {
                            check.set_active(true);
                        }
                        check.set_sensitive((!app.unfree || unfree.is_active()) && !needed);
                        check.set_tooltip_text(if needed { Some("Required by Rustup. Deselect Rustup to make this optional.") }
                            else if app.unfree && !unfree.is_active() { Some("Enable unfree software on Desktops & Wi-Fi to select this application.") }
                            else { None });
                    }
                    count.set_text(&format!(
                        "{} selected · Rustup also selects Development build tools",
                        checks.iter().filter(|check| check.is_active()).count()
                    ));
                }
            }
            updating.set(false);
        }
    });
    for check in &checks {
        check.connect_toggled({
            let refresh = refresh.clone();
            move |_| refresh()
        });
    }
    unfree.connect_toggled({
        let refresh = refresh.clone();
        move |_| refresh()
    });
    refresh();
    (page, checks)
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
        "Native Rust installer · your choice of desktops · pinned installation inputs",
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
        "This release supports erasing one whole disk: GPT with ext4, Btrfs or XFS, EFI or legacy BIOS. It does not support manual partitioning, encryption, preserving another OS, or offline installation. Back up your data before continuing.",
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
    let filesystem = DropDown::from_strings(&Filesystem::ALL.map(Filesystem::label));
    row(&grid, 6, "Root filesystem", &filesystem);
    setup.append(&grid);
    setup.append(&label("Btrfs uses compression on a single root volume; automatic snapshots are not configured. The EFI boot partition uses FAT32."));
    let location_page = GtkBox::new(Orientation::Vertical, 12);
    let location_grid = gtk::Grid::builder()
        .row_spacing(12)
        .column_spacing(18)
        .build();
    let locale = DropDown::from_strings(LOCALES);
    row(&location_grid, 0, "System locale", &locale);
    let timezone = entry("");
    timezone.set_placeholder_text(Some("Detecting… or enter America/Chicago"));
    timezone.set_max_length(100);
    row(&location_grid, 1, "Time zone", &timezone);
    let keyboard = DropDown::from_strings(KEYBOARDS);
    row(&location_grid, 2, "Installed keyboard layout", &keyboard);
    location_page.append(&location_grid);
    let internet_zone = CheckButton::with_label(
        "Use internet detection if the live time zone is unset (ipapi.co receives your public IP)",
    );
    internet_zone.set_active(true);
    location_page.append(&internet_zone);
    let detect = Button::with_label("Detect time zone again");
    let zone_spinner = Spinner::new();
    let detect_row = GtkBox::new(Orientation::Horizontal, 12);
    detect_row.append(&detect);
    detect_row.append(&zone_spinner);
    location_page.append(&detect_row);
    let zone_status = label("Checking the live system's time zone…");
    location_page.append(&zone_status);
    let zone_confirm =
        CheckButton::with_label("I have checked that this time zone is correct for my location");
    location_page.append(&zone_confirm);
    location_page.append(&label("US Central is America/Chicago; US Eastern is America/New_York. Region-based zones handle daylight saving automatically. Locale and country alone cannot determine your zone."));
    location_page.append(&label("The live keyboard layout is unchanged. Passwords are entered using the current live layout; confirm it before proceeding."));
    let desktop_page = GtkBox::new(Orientation::Vertical, 12);
    desktop_page.append(&label("Choose one or more desktop environments. All selected sessions will be available on the login screen; the live desktop stays Plasma."));
    let desktops: Vec<_> = Desktop::ALL
        .iter()
        .map(|desktop| {
            let check = CheckButton::with_label(desktop.label());
            check.set_active(*desktop == Desktop::Plasma);
            desktop_page.append(&check);
            check
        })
        .collect();
    let default_desktop = DropDown::from_strings(&Desktop::ALL.map(Desktop::label));
    desktop_page.append(&label("GNOME and Cinnamon are alternatives: the pinned NixOS modules cannot currently enable both together."));
    desktop_page.append(&label("Default login session"));
    desktop_page.append(&default_desktop);
    for (index, check) in desktops.iter().enumerate() {
        check.connect_toggled({
            let desktops = desktops.clone();
            let default_desktop = default_desktop.clone();
            move |check| {
                if check.is_active() && default_desktop.selected() == gtk::INVALID_LIST_POSITION {
                    default_desktop.set_selected(index as u32);
                }
                if !check.is_active() && default_desktop.selected() == index as u32 {
                    default_desktop.set_selected(
                        desktops
                            .iter()
                            .position(CheckButton::is_active)
                            .map(|n| n as u32)
                            .unwrap_or(gtk::INVALID_LIST_POSITION),
                    );
                }
            }
        });
    }
    default_desktop.connect_selected_notify({
        let desktops = desktops.clone();
        move |menu| {
            if let Some(check) = desktops.get(menu.selected() as usize) {
                check.set_active(true);
            }
        }
    });
    let copy_wifi = CheckButton::with_label(
        "Carry my saved live-session Wi-Fi connections and passwords into the installed system",
    );
    copy_wifi.set_active(true);
    desktop_page.append(&copy_wifi);
    desktop_page.append(&label("Wi-Fi profiles are stored root-only, outside the Nix store. Keep the live wallet unlocked. Enterprise networks using certificate files or hardware tokens must be configured after installation."));
    let unfree =
        CheckButton::with_label("Allow unfree software (some hardware drivers require this)");
    unfree.set_active(calamares_nixos::DEFAULT_ALLOW_UNFREE);
    desktop_page.append(&unfree);
    desktop_page.append(&label("Enabled by default. Redistributable firmware is included either way. Allowing unfree packages does not automatically configure every vendor driver."));
    let (applications_page, applications) = application_page(&unfree);
    let next = Button::with_label("Review installation");
    next.add_css_class("suggested-action");
    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);
    for (name, child) in [
        ("System & account", &setup),
        ("Desktops & Wi-Fi", &desktop_page),
        ("Location", &location_page),
        ("Applications", &applications_page),
    ] {
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(child)
            .build();
        // Notebook headers must keep their natural width. Wrapping body-text
        // labels can shrink these tabs enough to clip their last line.
        notebook.append_page(&scroll, Some(&Label::new(Some(name))));
    }
    let setup_page = GtkBox::new(Orientation::Vertical, 12);
    setup_page.append(&notebook);
    setup_page.append(&next);
    stack.add_named(&setup_page, Some("setup"));

    let review = GtkBox::new(Orientation::Vertical, 18);
    let summary = label("");
    summary.set_selectable(true);
    let review_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&summary)
        .build();
    review.append(&review_scroll);
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
    let progress_heading = label(
        "Installation in progress. Keep the computer connected to power and the network. Closing the installer is disabled until this operation finishes.",
    );
    progress_page.append(&progress_heading);
    let progress = ProgressBar::new();
    progress.set_show_text(true);
    progress_page.append(&progress);
    let done = Button::with_label("Close installer");
    done.set_visible(false);
    progress_page.append(&done);
    let failure_details = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    let failure_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&failure_details)
        .visible(false)
        .build();
    progress_page.append(&failure_scroll);
    stack.add_named(&progress_page, Some("progress"));
    stack.set_visible_child_name("setup");
    window.set_child(Some(&outer));

    let (send, receive) = mpsc::sync_channel::<Message>(64);
    let zone_epoch = Rc::new(Cell::new(0u64));
    let zone_running = Rc::new(Cell::new(false));
    timezone.connect_changed({
        let epoch = zone_epoch.clone();
        let confirm = zone_confirm.clone();
        move |_| {
            epoch.set(epoch.get().wrapping_add(1));
            confirm.set_active(false);
        }
    });
    let detect_zone: Rc<dyn Fn()> = Rc::new({
        let running = zone_running.clone();
        let send = send.clone();
        let epoch = zone_epoch.clone();
        let internet = internet_zone.clone();
        let spinner = zone_spinner.clone();
        let button = detect.clone();
        let status = zone_status.clone();
        move || {
            running.set(true);
            epoch.set(epoch.get().wrapping_add(1));
            let generation = epoch.get();
            let internet = internet.is_active();
            spinner.start();
            button.set_sensitive(false);
            status.set_text("Detecting time zone… you can still edit it manually.");
            let send = send.clone();
            thread::spawn(move || {
                let result = Settings::load()
                    .and_then(|s| timezone::detect(std::path::Path::new(&s.zoneinfo), internet))
                    .map_err(|e| format!("{e:#}"));
                let _ = send.send(Message::Zone(generation, result));
            });
        }
    });
    detect.connect_clicked({
        let detect_zone = detect_zone.clone();
        move |_| detect_zone()
    });
    internet_zone.connect_toggled({
        let epoch = zone_epoch.clone();
        move |_| epoch.set(epoch.get().wrapping_add(1))
    });
    let disks = Rc::new(RefCell::new(Vec::<Disk>::new()));
    let firmware = Rc::new(Cell::new(Firmware::Bios));
    let pending = Rc::new(RefCell::new(None::<InstallPlan>));
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
        let timezone = timezone.clone();
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
            if !zone_confirm.is_active() {
                notebook.set_current_page(Some(2));
                status.set_text(
                    "Check the time zone on the Location tab and confirm it before continuing.",
                );
                return;
            }
            let Some(root_filesystem) =
                Filesystem::ALL.get(filesystem.selected() as usize).copied()
            else {
                status.set_text("Select a root filesystem before continuing.");
                return;
            };
            let request = RawRequest {
                confirmation: String::new(),
                disk: disk.identity,
                firmware: firmware.get(),
                filesystem: root_filesystem,
                hostname: hostname.text().into(),
                username: username.text().into(),
                full_name: full_name.text().into(),
                password: password.text().into(),
                locale: choice(&locale, LOCALES),
                timezone: timezone.text().into(),
                keyboard: choice(&keyboard, KEYBOARDS),
                desktops: Desktop::ALL
                    .iter()
                    .zip(&desktops)
                    .filter_map(|(desktop, check)| check.is_active().then_some(*desktop))
                    .collect(),
                default_desktop: Desktop::ALL
                    .get(default_desktop.selected() as usize)
                    .copied()
                    .unwrap_or(Desktop::Plasma),
                applications: applications::catalog()
                    .iter()
                    .zip(&applications)
                    .filter_map(|(app, check)| check.is_active().then_some(app.id.clone()))
                    .collect(),
                copy_wifi: copy_wifi.is_active(),
                wifi_profiles: vec![],
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
                let result = (|| -> anyhow::Result<Box<InstallPlan>> {
                    let plan = request.parse(&Settings::load()?)?;
                    disk::revalidate(plan.disk())?;
                    Ok(Box::new(plan.snapshot_wifi()?))
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
                        .is_some_and(|r| erase.text() == format!("ERASE {}", r.disk().path)),
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
        let send = send.clone();
        let pending = pending.clone();
        let busy = busy.clone();
        let installing = installing.clone();
        let spinner = spinner.clone();
        let stack = stack.clone();
        let status = status.clone();
        let erase = erase.clone();
        move |_| {
            let Some(plan) = pending.borrow_mut().take() else { return; };
            let confirmed = match plan.confirm(erase.text().as_str()) {
                Ok(confirmed) => confirmed,
                Err(error) => {
                    stack.set_visible_child_name("setup");
                    status.set_text(&error.to_string());
                    return;
                }
            };
            password.set_text("");
            repeat.set_text("");
            busy.set(true);
            installing.set(true);
            spinner.start();
            stack.set_visible_child_name("progress");
            status.set_text("Waiting for authorization. The helper will repeat all safety checks before any disk write.");
            let send = send.clone();
            thread::spawn(move || {
                let outcome = launch(confirmed, send.clone()).map_err(|e| format!("{e:#}"));
                let _ = send.send(Message::Finished(outcome));
            });
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
                                let desktop_names = r
                                    .desktops()
                                    .selected()
                                    .iter()
                                    .map(|d| d.label())
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                let application_names = r.applications().names();
                                summary.set_text(&format!("ERASE ALL DATA ON {}\nModel: {} · Serial: {} · Size: {:.1} GiB\n\nDesktops: {}\nDefault session: {} · {:?} / {}\nHost: {} · User: {}\nLocale: {} · Time zone: {} · Keyboard: {}\nWi-Fi transfer: {} · {} saved profiles\nUnfree software: {}\nApplications: {}\n\nThe installation uses the media's pinned Determinate Nix flake. Root login is locked; your user can administer the system with sudo.\n\nType exactly: ERASE {}",r.disk().path,r.disk().model,r.disk().serial,r.disk().bytes as f64/1024f64.powi(3),desktop_names,r.desktops().default().label(),r.firmware(),r.filesystem().name(),r.hostname().as_str(),r.username().as_str(),r.locale(),r.timezone().as_str(),r.keyboard(),r.wifi().enabled(),r.wifi().profile_count(),r.allow_unfree(),if application_names.is_empty() { "None" } else { &application_names },r.disk().path));
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
                        failure_details.buffer().set_text(&message);
                        failure_scroll.set_visible(true);
                        status.set_text("Installation failed. See the details above; the disk may have been modified.");
                    }
                    Message::Event(Event::Complete) => {
                        progress.set_fraction(1.0);
                    }
                    Message::Finished(result) => {
                        busy.set(false);
                        installing.set(false);
                        spinner.stop();
                        done.set_visible(true);
                        progress_heading.set_text(if result.is_ok() {
                            "Installation complete."
                        } else {
                            "Installation did not complete. See the details below."
                        });
                        match result {Ok(())=>status.set_text("Installation complete. Shut down the live system, remove the media and boot the installed disk."),Err(e)=>{if !status.text().starts_with("Installation failed."){status.set_text(&e);}}}
                    }
                    Message::Zone(generation, result) => {
                        zone_running.set(false);
                        zone_spinner.stop();
                        detect.set_sensitive(true);
                        if generation == zone_epoch.get() {
                            match result {
                                Ok(found) => {
                                    timezone.set_text(found.zone.as_str());
                                    zone_status.set_text(&found.explanation);
                                }
                                Err(error) => zone_status.set_text(&error),
                            }
                        } else {
                            zone_status.set_text("Your manual choice was kept; the detection result was not applied.");
                        }
                    }
                }
            }
            // Keep a visible activity indicator even when Location is not the
            // selected tab while its worker is still detecting the time zone.
            if busy.get() || zone_running.get() {
                spinner.start();
            } else {
                spinner.stop();
            }
            glib::ControlFlow::Continue
        }
    });
    window.present();
    rescan();
    detect_zone();
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
