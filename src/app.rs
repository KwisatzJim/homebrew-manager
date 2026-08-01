use crate::brew::{self, BrewStatus, Event, OutdatedPackage, Package};
use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(PartialEq, Clone, Copy)]
enum View {
    Status,
    Installed,
    Search,
    Updates,
    Maintenance,
}

pub struct HomebrewManagerApp {
    view: View,

    // Channel shared by every background job. Any long-running brew
    // operation clones `tx` and sends `Event`s back here; `update()` drains
    // `rx` once per frame.
    tx: Sender<Event>,
    rx: Receiver<Event>,

    status: BrewStatus,
    status_checked: bool,
    busy: bool,
    current_op: Option<String>,

    installed_formulae: Vec<Package>,
    installed_casks: Vec<Package>,
    installed_filter: String,
    selected_installed: HashSet<String>,

    search_query: String,
    search_results: Vec<String>,
    selected_search: HashSet<String>,
    install_as_cask: bool,

    outdated: Vec<OutdatedPackage>,
    selected_outdated: HashSet<String>,

    info_name: Option<String>,
    info_text: String,

    log: Vec<String>,
    autoscroll_log: bool,
}

impl HomebrewManagerApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        // Kick off an initial status check right away.
        brew::refresh_status_async(tx.clone());
        Self {
            view: View::Status,
            tx,
            rx,
            status: BrewStatus::default(),
            status_checked: false,
            busy: false,
            current_op: None,
            installed_formulae: Vec::new(),
            installed_casks: Vec::new(),
            installed_filter: String::new(),
            selected_installed: HashSet::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            selected_search: HashSet::new(),
            install_as_cask: false,
            outdated: Vec::new(),
            selected_outdated: HashSet::new(),
            info_name: None,
            info_text: String::new(),
            log: Vec::new(),
            autoscroll_log: true,
        }
    }

    fn brew_path(&self) -> Option<String> {
        self.status.brew_path.clone()
    }

    /// Start a background job and mark the UI busy. `label` is shown while
    /// it runs. The job itself is responsible for sending its own
    /// `Event::Finished`.
    fn start_job(&mut self, label: &str, job: impl FnOnce(Sender<Event>)) {
        self.busy = true;
        self.current_op = Some(label.to_string());
        self.log.push(format!("$ {label}"));
        job(self.tx.clone());
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        let mut got_any = false;
        while let Ok(event) = self.rx.try_recv() {
            got_any = true;
            match event {
                Event::Log(line) => self.log.push(line),
                Event::Finished { success, exit_code } => {
                    self.busy = false;
                    let op = self.current_op.take().unwrap_or_default();
                    let summary = match exit_code {
                        Some(code) => format!("[{op}] finished (exit {code}, success={success})"),
                        None => format!("[{op}] finished (success={success})"),
                    };
                    self.log.push(summary);
                    // Auto-refresh relevant views after mutating operations.
                    if success {
                        if let Some(path) = self.brew_path() {
                            match op.as_str() {
                                "Install Homebrew" => brew::refresh_status_async(self.tx.clone()),
                                "Install" | "Uninstall" | "Upgrade" | "Upgrade All"
                                | "Cleanup" | "Autoremove" => {
                                    brew::refresh_installed(path.clone(), self.tx.clone());
                                    brew::refresh_outdated(path, self.tx.clone());
                                }
                                "Update (brew update)" => {
                                    brew::refresh_outdated(path, self.tx.clone());
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Event::Status(status) => {
                    self.status_checked = true;
                    let now_installed = status.installed;
                    self.status = status;
                    if now_installed {
                        if let Some(path) = self.brew_path() {
                            brew::refresh_installed(path.clone(), self.tx.clone());
                            brew::refresh_outdated(path, self.tx.clone());
                        }
                    }
                }
                Event::InstalledFormulae(pkgs) => self.installed_formulae = pkgs,
                Event::InstalledCasks(pkgs) => self.installed_casks = pkgs,
                Event::Outdated(list) => self.outdated = list,
                Event::SearchResults(results) => self.search_results = results,
                Event::Info(name, text) => {
                    self.info_name = Some(name);
                    self.info_text = text;
                }
            }
        }
        if got_any {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for HomebrewManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events(ctx);
        // Keep polling while a background job is in flight so the log
        // updates smoothly even without new input events.
        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::SidePanel::left("nav").resizable(false).show(ctx, |ui| {
            ui.add_space(8.0);
            ui.heading("🍺 Homebrew Manager");
            ui.add_space(12.0);
            ui.selectable_value(&mut self.view, View::Status, "Status");
            ui.selectable_value(&mut self.view, View::Installed, "Installed");
            ui.selectable_value(&mut self.view, View::Search, "Search / Install");
            ui.selectable_value(&mut self.view, View::Updates, "Updates");
            ui.selectable_value(&mut self.view, View::Maintenance, "Maintenance");
            ui.add_space(12.0);
            ui.separator();
            if self.status.installed {
                ui.label(egui::RichText::new("✔ brew installed").color(egui::Color32::from_rgb(80, 180, 80)));
                if !self.status.version.is_empty() {
                    ui.small(&self.status.version);
                }
            } else if self.status_checked {
                ui.label(egui::RichText::new("✘ brew not found").color(egui::Color32::from_rgb(200, 80, 80)));
            } else {
                ui.label("Checking…");
            }
            if self.busy {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(self.current_op.clone().unwrap_or_else(|| "Working…".to_string()));
                });
            }
        });

        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(180.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Console");
                    if ui.button("Clear").clicked() {
                        self.log.clear();
                    }
                    ui.checkbox(&mut self.autoscroll_log, "Autoscroll");
                });
                egui::ScrollArea::vertical()
                    .stick_to_bottom(self.autoscroll_log)
                    .max_height(140.0)
                    .show(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        for line in &self.log {
                            ui.monospace(line);
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| match self.view {
            View::Status => self.ui_status(ui),
            View::Installed => self.ui_installed(ui),
            View::Search => self.ui_search(ui),
            View::Updates => self.ui_updates(ui),
            View::Maintenance => self.ui_maintenance(ui),
        });

        if let Some(name) = self.info_name.clone() {
            let mut open = true;
            egui::Window::new(format!("brew info: {name}"))
                .open(&mut open)
                .default_width(500.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                        ui.monospace(&self.info_text);
                    });
                });
            if !open {
                self.info_name = None;
            }
        }
    }
}

impl HomebrewManagerApp {
    fn ui_status(&mut self, ui: &mut egui::Ui) {
        ui.heading("Homebrew Status");
        ui.add_space(8.0);

        if ui.button("Re-check").clicked() && !self.busy {
            self.status_checked = false;
            brew::refresh_status_async(self.tx.clone());
        }

        ui.add_space(12.0);

        if self.status.installed {
            egui::Grid::new("status_grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Installed:");
                ui.label("Yes");
                ui.end_row();
                ui.label("Version:");
                ui.label(&self.status.version);
                ui.end_row();
                ui.label("Prefix:");
                ui.label(&self.status.prefix);
                ui.end_row();
                ui.label("Binary:");
                ui.label(self.status.brew_path.clone().unwrap_or_default());
                ui.end_row();
            });
        } else if self.status_checked {
            ui.colored_label(egui::Color32::from_rgb(200, 80, 80), "Homebrew was not found on this system.");
            ui.add_space(8.0);
            ui.label(
                "Clicking Install will run the official Homebrew installer \
                 (curl | bash) with NONINTERACTIVE=1. Watch the console below \
                 for progress. Note: on some systems the installer may still \
                 invoke sudo for certain steps (e.g. Xcode Command Line \
                 Tools), which can hang waiting for a password if this app \
                 has no terminal attached — if that happens, run the install \
                 from a real terminal instead.",
            );
            ui.add_space(8.0);
            if ui.add_enabled(!self.busy, egui::Button::new("Install Homebrew")).clicked() {
                self.start_job("Install Homebrew", |tx| brew::install_homebrew(tx));
            }
        } else {
            ui.label("Checking for Homebrew…");
        }
    }

    fn ui_installed(&mut self, ui: &mut egui::Ui) {
        let Some(brew_path) = self.brew_path() else {
            ui.label("Homebrew is not installed yet — see the Status tab.");
            return;
        };

        ui.horizontal(|ui| {
            ui.heading("Installed Packages");
            if ui.add_enabled(!self.busy, egui::Button::new("Refresh")).clicked() {
                brew::refresh_installed(brew_path.clone(), self.tx.clone());
            }
        });
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.installed_filter);
        });
        ui.add_space(6.0);

        let selected_count = self.selected_installed.len();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!self.busy && selected_count > 0, |ui| {
                if ui.button(format!("Uninstall selected ({selected_count})")).clicked() {
                    let formulae: Vec<String> = self
                        .selected_installed
                        .iter()
                        .filter(|n| self.installed_formulae.iter().any(|p| &p.name == *n))
                        .cloned()
                        .collect();
                    let casks: Vec<String> = self
                        .selected_installed
                        .iter()
                        .filter(|n| self.installed_casks.iter().any(|p| &p.name == *n))
                        .cloned()
                        .collect();
                    let path = brew_path.clone();
                    self.selected_installed.clear();
                    self.start_job("Uninstall", move |tx| {
                        if !formulae.is_empty() {
                            brew::uninstall_packages(path.clone(), formulae, false, tx.clone());
                        }
                        if !casks.is_empty() {
                            brew::uninstall_packages(path, casks, true, tx);
                        }
                    });
                }
            });
            ui.add_enabled_ui(!self.busy && selected_count > 0, |ui| {
                if ui.button(format!("Upgrade selected ({selected_count})")).clicked() {
                    let names: Vec<String> = self.selected_installed.iter().cloned().collect();
                    let path = brew_path.clone();
                    self.selected_installed.clear();
                    self.start_job("Upgrade", move |tx| brew::upgrade_packages(path, names, tx));
                }
            });
        });

        ui.add_space(8.0);
        let filter = self.installed_filter.to_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(egui::RichText::new("Formulae").strong());
            for pkg in self.installed_formulae.clone() {
                if !filter.is_empty() && !pkg.name.to_lowercase().contains(&filter) {
                    continue;
                }
                self.package_row(ui, &pkg, &brew_path);
            }
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Casks").strong());
            for pkg in self.installed_casks.clone() {
                if !filter.is_empty() && !pkg.name.to_lowercase().contains(&filter) {
                    continue;
                }
                self.package_row(ui, &pkg, &brew_path);
            }
        });
    }

    fn package_row(&mut self, ui: &mut egui::Ui, pkg: &Package, brew_path: &str) {
        ui.horizontal(|ui| {
            let mut checked = self.selected_installed.contains(&pkg.name);
            if ui.checkbox(&mut checked, "").changed() {
                if checked {
                    self.selected_installed.insert(pkg.name.clone());
                } else {
                    self.selected_installed.remove(&pkg.name);
                }
            }
            ui.label(&pkg.name);
            ui.weak(&pkg.version);
            if ui.small_button("info").clicked() {
                brew::info(brew_path.to_string(), pkg.name.clone(), self.tx.clone());
            }
        });
    }

    fn ui_search(&mut self, ui: &mut egui::Ui) {
        let Some(brew_path) = self.brew_path() else {
            ui.label("Homebrew is not installed yet — see the Status tab.");
            return;
        };

        ui.heading("Search Available Packages");
        ui.horizontal(|ui| {
            let resp = ui.text_edit_singleline(&mut self.search_query);
            let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Search").clicked() || enter_pressed) && !self.search_query.trim().is_empty() {
                brew::search(brew_path.clone(), self.search_query.trim().to_string(), self.tx.clone());
            }
            ui.checkbox(&mut self.install_as_cask, "Install as cask");
        });

        ui.add_space(8.0);
        let selected_count = self.selected_search.len();
        ui.add_enabled_ui(!self.busy && selected_count > 0, |ui| {
            if ui.button(format!("Install selected ({selected_count})")).clicked() {
                let names: Vec<String> = self.selected_search.iter().cloned().collect();
                let path = brew_path.clone();
                let cask = self.install_as_cask;
                self.selected_search.clear();
                self.start_job("Install", move |tx| brew::install_packages(path, names, cask, tx));
            }
        });

        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for name in self.search_results.clone() {
                ui.horizontal(|ui| {
                    let mut checked = self.selected_search.contains(&name);
                    if ui.checkbox(&mut checked, "").changed() {
                        if checked {
                            self.selected_search.insert(name.clone());
                        } else {
                            self.selected_search.remove(&name);
                        }
                    }
                    ui.label(&name);
                    if ui.small_button("info").clicked() {
                        brew::info(brew_path.clone(), name.clone(), self.tx.clone());
                    }
                    if ui.small_button("install").clicked() && !self.busy {
                        let path = brew_path.clone();
                        let n = name.clone();
                        let cask = self.install_as_cask;
                        self.start_job("Install", move |tx| brew::install_packages(path, vec![n], cask, tx));
                    }
                });
            }
        });
    }

    fn ui_updates(&mut self, ui: &mut egui::Ui) {
        let Some(brew_path) = self.brew_path() else {
            ui.label("Homebrew is not installed yet — see the Status tab.");
            return;
        };

        ui.heading("Updates");
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.busy, egui::Button::new("Check for updates (brew update)")).clicked() {
                let path = brew_path.clone();
                self.start_job("Update (brew update)", move |tx| brew::update(path, tx));
            }
            if ui.add_enabled(!self.busy, egui::Button::new("Refresh outdated list")).clicked() {
                brew::refresh_outdated(brew_path.clone(), self.tx.clone());
            }
            if ui.add_enabled(!self.busy && !self.outdated.is_empty(), egui::Button::new("Upgrade All")).clicked() {
                let path = brew_path.clone();
                self.start_job("Upgrade All", move |tx| brew::upgrade_all(path, tx));
            }
        });

        ui.add_space(8.0);
        let selected_count = self.selected_outdated.len();
        ui.add_enabled_ui(!self.busy && selected_count > 0, |ui| {
            if ui.button(format!("Upgrade selected ({selected_count})")).clicked() {
                let names: Vec<String> = self.selected_outdated.iter().cloned().collect();
                let path = brew_path.clone();
                self.selected_outdated.clear();
                self.start_job("Upgrade", move |tx| brew::upgrade_packages(path, names, tx));
            }
        });

        ui.add_space(8.0);
        if self.outdated.is_empty() {
            ui.label("Everything is up to date (or you haven't checked yet).");
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for pkg in self.outdated.clone() {
                ui.horizontal(|ui| {
                    let mut checked = self.selected_outdated.contains(&pkg.name);
                    if ui.checkbox(&mut checked, "").changed() {
                        if checked {
                            self.selected_outdated.insert(pkg.name.clone());
                        } else {
                            self.selected_outdated.remove(&pkg.name);
                        }
                    }
                    ui.label(&pkg.name);
                    ui.weak(format!("{} → {}", pkg.current, pkg.latest));
                    if pkg.pinned {
                        ui.colored_label(egui::Color32::from_rgb(200, 160, 60), "pinned");
                    }
                    if pkg.pinned {
                        if ui.small_button("unpin").clicked() {
                            let path = brew_path.clone();
                            let n = pkg.name.clone();
                            self.start_job("Unpin", move |tx| brew::pin_packages(path, vec![n], false, tx));
                        }
                    } else if ui.small_button("pin").clicked() {
                        let path = brew_path.clone();
                        let n = pkg.name.clone();
                        self.start_job("Pin", move |tx| brew::pin_packages(path, vec![n], true, tx));
                    }
                });
            }
        });
    }

    fn ui_maintenance(&mut self, ui: &mut egui::Ui) {
        let Some(brew_path) = self.brew_path() else {
            ui.label("Homebrew is not installed yet — see the Status tab.");
            return;
        };

        ui.heading("Maintenance");
        ui.add_space(8.0);
        ui.label("These map directly to their brew CLI equivalents; output streams to the console below.");
        ui.add_space(12.0);

        ui.add_enabled_ui(!self.busy, |ui| {
            if ui.button("brew cleanup -s").clicked() {
                let path = brew_path.clone();
                self.start_job("Cleanup", move |tx| brew::cleanup(path, tx));
            }
            ui.add_space(4.0);
            if ui.button("brew autoremove").clicked() {
                let path = brew_path.clone();
                self.start_job("Autoremove", move |tx| brew::autoremove(path, tx));
            }
            ui.add_space(4.0);
            if ui.button("brew doctor").clicked() {
                let path = brew_path.clone();
                self.start_job("Doctor", move |tx| brew::doctor(path, tx));
            }
            ui.add_space(4.0);
            if ui.button("brew update").clicked() {
                let path = brew_path.clone();
                self.start_job("Update (brew update)", move |tx| brew::update(path, tx));
            }
        });
    }
}
