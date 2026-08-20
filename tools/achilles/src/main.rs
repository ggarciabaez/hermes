mod decoders;
mod liveliness;
mod comms;
mod message_viewer;
mod mode;
mod model;
mod params;
mod shape;

use std::sync::Arc;

use eframe::egui;
use eframe::App as _;
use egui_graphs::{GraphChange, GraphView, GraphViewResponse, SettingsInteraction, SettingsNavigation, SettingsStyle};
use hermes_types::sm::Mode;
use petgraph::graph::NodeIndex;
use petgraph::stable_graph::DefaultIx;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use liveliness::{extract_domain, extract_node, handle_liveliness, LivelinessChange};
use comms::{spawn_refresh as spawn_comms_refresh, CommsEvent, CommsPanel};
use message_viewer::MessageViewer;
use mode::{handle_mode, spawn_set_mode, ModeEvent, ModePanel};
use model::GraphModel;
use params::{handle_params_changed, parse_param_value, spawn_refresh, spawn_set, format_param_value, format_bounds, ParamEvent, ParamsPanel};

fn gen_force_view(interactions: &SettingsInteraction, styles: &SettingsStyle, navs: &SettingsNavigation) -> GraphView<egui_graphs::FruchtermanReingoldWithCenterGravityState, egui_graphs::LayoutForceDirected<egui_graphs::FruchtermanReingoldWithCenterGravity>> {
    type L = egui_graphs::LayoutForceDirected<egui_graphs::FruchtermanReingoldWithCenterGravity>;
    type S = egui_graphs::FruchtermanReingoldWithCenterGravityState;
    GraphView::<S, L>::new()
        .with_interactions(interactions)
        .with_styles(styles)
        .with_navigations(navs)
}

fn gen_default_view(interactions: &SettingsInteraction, styles: &SettingsStyle, navs: &SettingsNavigation) -> egui_graphs::DefaultGraphView {
    egui_graphs::DefaultGraphView::new()
        .with_interactions(interactions)
        .with_styles(styles)
        .with_navigations(navs)
}

/// Render a `label -> count` map as `label=count, ...` sorted by label.
fn sorted_counts(m: &std::collections::HashMap<String, u64>) -> String {
    let mut items: Vec<_> = m.iter().map(|(k, v)| (k.clone(), *v)).collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("  ")
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum RightTab {
    Inspector,
    Params,
    Comms,
}

struct AchillesApp {
    model: GraphModel,
    selected: Option<NodeIndex>,
    force: bool,
    fit_to_screen: bool,
    left_open: bool,
    right_open: bool,
    right_tab: RightTab,
    status: String,
    live_channel: Option<UnboundedReceiver<LivelinessChange>>,
    live_handle: Option<tokio::task::JoinHandle<()>>,
    mode_channel: Option<UnboundedReceiver<ModeEvent>>,
    mode_handle: Option<tokio::task::JoinHandle<()>>,
    mode_sender: UnboundedSender<ModeEvent>,
    mode_panel: ModePanel,
    param_channel: Option<UnboundedReceiver<ParamEvent>>,
    param_handle: Option<tokio::task::JoinHandle<()>>,
    param_sender: UnboundedSender<ParamEvent>,
    params_panel: ParamsPanel,
    comms_channel: Option<UnboundedReceiver<CommsEvent>>,
    comms_sender: UnboundedSender<CommsEvent>,
    comms_panel: CommsPanel,
    viewer: MessageViewer,
    session: zenoh::Session,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl AchillesApp {
    #[allow(clippy::too_many_arguments)]
    fn new(
        live_channel: UnboundedReceiver<LivelinessChange>,
        live_handle: tokio::task::JoinHandle<()>,
        mode_channel: UnboundedReceiver<ModeEvent>,
        mode_handle: tokio::task::JoinHandle<()>,
        mode_sender: UnboundedSender<ModeEvent>,
        param_channel: UnboundedReceiver<ParamEvent>,
        param_handle: tokio::task::JoinHandle<()>,
        param_sender: UnboundedSender<ParamEvent>,
        comms_channel: UnboundedReceiver<CommsEvent>,
        comms_sender: UnboundedSender<CommsEvent>,
        session: zenoh::Session,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            model: GraphModel::new(),
            selected: None,
            force: false,
            fit_to_screen: false,
            left_open: true,
            right_open: true,
            right_tab: RightTab::Inspector,
            status: String::new(),
            live_channel: Some(live_channel),
            live_handle: Some(live_handle),
            mode_channel: Some(mode_channel),
            mode_handle: Some(mode_handle),
            mode_sender,
            mode_panel: ModePanel::new(),
            param_channel: Some(param_channel),
            param_handle: Some(param_handle),
            param_sender,
            params_panel: ParamsPanel::new(),
            comms_channel: Some(comms_channel),
            comms_sender,
            comms_panel: CommsPanel::new(),
            viewer: MessageViewer::new(),
            session,
            runtime,
        }
    }

    fn drain_events(&mut self) {
        while let Ok(change) = self.live_channel.as_mut().unwrap().try_recv() {
            match change {
                LivelinessChange::Created { name, manifest } => {
                    let (_ns, domain, _ep) = extract_domain(&name);
                    self.model.upsert_domain(&domain, manifest);
                    self.model.reconcile_edges();
                }
                LivelinessChange::ManifestUpdated { name, manifest } => {
                    let (_ns, domain, _ep) = extract_domain(&name);
                    self.model.upsert_domain(&domain, manifest);
                    self.model.reconcile_edges();
                }
                LivelinessChange::Dropped(name) => {
                    let (_ns, domain, _ep) = extract_domain(&name);
                    self.model.remove_domain(&domain);
                }
            }
        }
        // Close the message viewer if its topic no longer exists on the graph.
        if self.viewer.is_open()
            && let Some(t) = self.viewer.topic.clone()
            && !self.model.live_pub_topics().contains(&t)
        {
            self.viewer.close();
        }

        while let Ok(evt) = self.mode_channel.as_mut().unwrap().try_recv() {
            match evt {
                ModeEvent::Current(m) => self.mode_panel.mode = m,
                ModeEvent::SetResult { mode, result } => {
                    self.mode_panel.status = Some(match result {
                        Ok(_) => format!("set {} ok", mode),
                        Err(e) => format!("set {} failed: {}", mode, e),
                    });
                }
            }
        }

        while let Ok(evt) = self.param_channel.as_mut().unwrap().try_recv() {
            match evt {
                ParamEvent::Loaded(rows) => self.params_panel.apply_loaded(rows),
                ParamEvent::SetResult { domain, key, result } => {
                    self.params_panel.status = Some(match result {
                        Ok(_) => format!("{}: {} set ok", domain, key),
                        Err(e) => format!("{}: {} set failed: {}", domain, key, e),
                    });
                }
                ParamEvent::Changed { domain, info } => {
                    self.params_panel.apply_changed(&domain, info);
                }
            }
        }

        while let Ok(evt) = self.comms_channel.as_mut().unwrap().try_recv() {
            match evt {
                CommsEvent::Loaded(snap) => self.comms_panel.apply(snap),
                CommsEvent::Failed(e) => {
                    self.comms_panel.loading = false;
                    self.comms_panel.status = Some(e);
                }
            }
        }
    }

    fn render_top_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_panel").show(ui, |ui| {
            egui::MenuBar::default().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Exit").clicked() {
                        self.on_exit();
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.left_open, "Mode & Graph panel");
                    ui.checkbox(&mut self.right_open, "Inspector / Params panel");
                })
            })
        });
    }

    fn render_left_panel(&mut self, ui: &mut egui::Ui) {
        let mut left_open = self.left_open;
        egui::Panel::left("mode_panel")
            .resizable(true)
            .default_size(180.0)
            .show_collapsible(ui, &mut left_open, |ui| {
                ui.heading("Mode Control");
                ui.separator();

                let m = self.mode_panel.mode;
                ui.label(
                    egui::RichText::new(format!("Current: {}", m))
                        .size(18.0)
                        .strong(),
                );
                if let Some(status) = &self.mode_panel.status {
                    ui.small(status);
                }

                ui.separator();
                ui.label("Set mode:");
                for mode in [Mode::Standby, Mode::Manual, Mode::Auto, Mode::Halt] {
                    if ui
                        .button(if mode == m { format!("{} ✓", mode) } else { format!("{}", mode) })
                        .clicked()
                    {
                        spawn_set_mode(
                            self.session.clone(),
                            &self.runtime,
                            self.mode_sender.clone(),
                            mode,
                        );
                    }
                }

                ui.separator();
                ui.heading("Graph");
                ui.checkbox(&mut self.force, "Use Force on Graph");
                ui.checkbox(&mut self.fit_to_screen, "Auto-fit to screen");
                ui.horizontal(|ui| {
                    if ui.button("Clear selection").clicked() {
                        self.selected = None;
                    }
                });

                ui.separator();
                let (fps, dt_ms) = ui.input(|i| {
                    (1.0 / i.stable_dt.max(1e-6), i.stable_dt * 1000.0)
                });
                ui.small(format!("{:.0} fps | {:.2} ms/frame", fps, dt_ms));

                if !self.status.is_empty() {
                    ui.separator();
                    ui.small(&self.status);
                }
            });
        self.left_open = left_open;
    }

    fn render_right_panel(&mut self, ui: &mut egui::Ui) {
        let mut right_open = self.right_open;
        egui::Panel::right("inspector_panel")
            .resizable(true)
            .default_size(300.0)
            .show_collapsible(ui, &mut right_open, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.right_tab, RightTab::Inspector, "Inspector");
                    ui.selectable_value(&mut self.right_tab, RightTab::Params, "Params");
                    ui.selectable_value(&mut self.right_tab, RightTab::Comms, "Comms");
                });
                ui.separator();
                match self.right_tab {
                    RightTab::Inspector => self.render_inspector(ui),
                    RightTab::Params => self.render_params(ui),
                    RightTab::Comms => self.render_comms(ui),
                }
            });
        self.right_open = right_open;
    }

    fn render_inspector(&mut self, ui: &mut egui::Ui) {
        let mut viewer_open_topic: Option<String> = None;

        ui.heading("Node Inspector");
        ui.separator();

        match self.selected {
            None => {
                ui.label("Click a node to inspect it.");
            }
            Some(idx) => {
                if let Some((domain, rec)) = self.model.domain_by_node(idx) {
                    ui.strong(format!("Domain: {}", domain));
                    ui.small(format!(
                        "{} endpoints ({} publishes, {} serves)",
                        rec.endpoints.len(),
                        rec.manifest.publishes.len(),
                        rec.manifest.serves.len()
                    ));

                    ui.separator();
                    ui.label("Publishes:");
                    for key in &rec.manifest.publishes {
                        ui.horizontal(|ui| {
                            ui.monospace(key);
                            if self.viewer.is_open_for(key) {
                                ui.weak("viewing");
                            } else if ui.button("View").clicked() {
                                viewer_open_topic = Some(key.clone());
                            }
                        });
                    }
                    ui.separator();
                    ui.label("Serves:");
                    for key in &rec.manifest.serves {
                        ui.monospace(key);
                    }
                    ui.separator();
                    ui.label("Subscribes:");
                    for key in &rec.manifest.subscribes {
                        ui.monospace(key);
                    }
                } else if let Some((key, rec)) = self.model.endpoint_by_node(idx) {
                    ui.strong(format!("Endpoint: {}", key));
                    ui.small(format!("owner: {} | kind: {:?}", rec.owner, rec.kind));
                    ui.separator();
                    if matches!(&rec.kind, model::EndpointKind::Publisher) {
                        if self.viewer.is_open_for(key) {
                            ui.weak("viewing");
                        } else if ui.button("Open Message Viewer").clicked() {
                            viewer_open_topic = Some(key.clone());
                        }
                    } else {
                        ui.label("Queryable endpoint (click on a domain to see serves).");
                    }
                } else {
                    ui.label("Selected node not found.");
                }
            }
        }

        ui.separator();
        ui.heading("Message Viewer");
        if let Some(topic) = viewer_open_topic {
            self.viewer
                .open(self.session.clone(), &self.runtime, ui.ctx().clone(), topic.clone());
            self.status = format!("viewing {}", topic);
        }

        if !self.viewer.is_open() {
            ui.label("No viewer open. Open one from a publisher or domain.");
            return;
        }

        let topic = self.viewer.topic.clone().unwrap();
        let mut close = false;
        ui.horizontal(|ui| {
            ui.strong(&topic);
            if ui.button("Stop").clicked() {
                close = true;
            }
            ui.small(format!("{} messages received", self.viewer.received()));
        });

        let latest = self.viewer.take_latest();
        match latest {
            Some(e) => {
                ui.monospace(format!("{} {}: {}", e.time, e.key, e.value));
            }
            None => {
                ui.label("Waiting for the first message...");
            }
        }

        if close {
            self.viewer.close();
        }
    }

    fn render_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("Parameters");
        ui.separator();

        let standby = self.mode_panel.mode == Mode::Standby;
        if !standby {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Editing locked: switch to Standby to modify parameters.",
            );
        }

        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.params_panel.loading = true;
                self.params_panel.status = None;
                spawn_refresh(self.session.clone(), &self.runtime, self.param_sender.clone());
            }
            if self.params_panel.loading {
                ui.spinner();
            }
        });
        if let Some(status) = &self.params_panel.status {
            ui.small(status);
        }
        ui.separator();

        if self.params_panel.rows.is_empty() {
            ui.label("No parameters discovered. Press Refresh.");
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut seen_domains: Vec<String> = Vec::new();
            let mut pending_sets: Vec<(String, String, String)> = Vec::new();

            let rows = self.params_panel.rows.clone();
            for row in &rows {
                if !seen_domains.contains(&row.domain) {
                    seen_domains.push(row.domain.clone());
                    ui.separator();
                    ui.strong(row.domain.to_string());
                }
                let key = format!("{}:{}", row.domain, row.info.name);
                let edit_value = self
                    .params_panel
                    .edits
                    .entry(key.clone())
                    .or_insert_with(|| format_param_value(&row.info.value))
                    .clone();

                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(&row.info.name)
                            .monospace()
                            .strong(),
                    );
                    ui.monospace(format_param_value(&row.info.value));
                    ui.small(format!("bounds {}", format_bounds(&row.info.bounds)));
                });
                ui.horizontal(|ui| {
                    ui.add_enabled(
                        standby,
                        egui::TextEdit::singleline(
                            self.params_panel.edits.entry(key.clone()).or_default(),
                        )
                        .desired_width(120.0),
                    );
                    if ui.add_enabled(standby, egui::Button::new("Set")).clicked() {
                        pending_sets.push((row.domain.clone(), row.info.name.clone(), edit_value));
                    }
                });
            }

            for (domain, name, text) in pending_sets {
                if let Some(row) = self.params_panel.row(&domain, &name) {
                    let info = row.info.clone();
                    match parse_param_value(&info, &text) {
                        Ok(value) => {
                            spawn_set(
                                self.session.clone(),
                                &self.runtime,
                                self.param_sender.clone(),
                                domain,
                                name,
                                value,
                            );
                        }
                        Err(e) => self.params_panel.status = Some(e),
                    }
                }
            }
        });
    }

    fn render_comms(&mut self, ui: &mut egui::Ui) {
        ui.heading("Comms");
        ui.separator();

        if self.comms_panel.due_for_poll() {
            self.comms_panel.poll_started();
            spawn_comms_refresh(
                self.session.clone(),
                &self.runtime,
                self.comms_sender.clone(),
            );
        }

        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.comms_panel.poll_started();
                spawn_comms_refresh(
                    self.session.clone(),
                    &self.runtime,
                    self.comms_sender.clone(),
                );
            }
            if self.comms_panel.loading {
                ui.spinner();
            }
        });
        if let Some(status) = &self.comms_panel.status {
            ui.small(status);
        }
        ui.separator();

        let Some(snap) = &self.comms_panel.snapshot else {
            ui.label("No data yet. Collecting on first visit to this tab.");
            return;
        };

        ui.label(format!("{} peer(s) reporting", snap.peers.len()));
        let mb = |b: u64| b as f64 / 1_000_000.0;
        ui.monospace(format!(
            "tx {:.2} MB | rx {:.2} MB",
            mb(snap.total_tx()),
            mb(snap.total_rx())
        ));
        if let Some((tx_rate, rx_rate)) = self.comms_panel.rates() {
            ui.monospace(format!(
                "tx {:.1} kB/s | rx {:.1} kB/s",
                tx_rate / 1000.0,
                rx_rate / 1000.0
            ));
        }
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for p in &snap.peers {
                ui.collapsing(
                    format!("{} ({})", comms::short_zid(&p.zid), p.whatami),
                    |ui| {
                        ui.monospace(format!("tx {} B | rx {} B", p.tx_bytes, p.rx_bytes));
                        ui.small(format!(
                            "transports {} | links {} | resources {}",
                            p.transports, p.links, p.resources
                        ));
                        if !p.tx_msgs.is_empty() {
                            ui.label("tx messages:");
                            ui.monospace(sorted_counts(&p.tx_msgs));
                        }
                        if !p.rx_msgs.is_empty() {
                            ui.label("rx messages:");
                            ui.monospace(sorted_counts(&p.rx_msgs));
                        }
                        if !p.tx_dropped.is_empty() || !p.rx_dropped.is_empty() {
                            ui.colored_label(egui::Color32::YELLOW, "drops:");
                            if !p.tx_dropped.is_empty() {
                                ui.monospace(format!("tx {}", sorted_counts(&p.tx_dropped)));
                            }
                            if !p.rx_dropped.is_empty() {
                                ui.monospace(format!("rx {}", sorted_counts(&p.rx_dropped)));
                            }
                        }
                    },
                );
            }
        });
    }

    fn render_graph(&mut self, ui: &mut egui::Ui) {
        let interactions = SettingsInteraction::default()
            .with_node_clicking_enabled(true)
            .with_edge_clicking_enabled(true)
            .with_edge_selection_enabled(true)
            .with_node_selection_enabled(true);
        let navs = SettingsNavigation::default()
            .with_fit_to_screen_enabled(self.fit_to_screen)
            .with_zoom_and_pan_enabled(true);
        let styles = SettingsStyle::new()
            .with_labels_always(true)
            .with_edge_stroke_hook(|selected, _order, default_stroke, _egui_style| {
                let mut stroke = default_stroke;
                stroke.width = if selected { 2.5 } else { 1.0 };
                stroke
            });

        let result: GraphViewResponse<DefaultIx> = if self.force {
            gen_force_view(&interactions, &styles, &navs).show(ui, self.model.g_mut())
        } else {
            gen_default_view(&interactions, &styles, &navs).show(ui, self.model.g_mut())
        };

        for change in &result.changes {
            if let Some(idx) = extract_node(change) {
                match change {
                    GraphChange::NodeSelected { .. }
                    | GraphChange::NodeClicked { .. }
                    | GraphChange::NodeDoubleClicked { .. } => self.selected = Some(idx),
                    GraphChange::NodeDeselected { .. }
                        if self.selected == Some(idx) =>
                    {
                        self.selected = None;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn main() -> eframe::Result {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Failed to create runtime");
    let session = runtime
        .block_on(async {
            zenoh::open(hermes_types::config::default_config())
                .await
        })
        .expect("Failed to open Zenoh session");

    let (live_tx, live_rx) = unbounded_channel::<LivelinessChange>();
    let live_handle = runtime.spawn(handle_liveliness(session.clone(), live_tx));

    let (mode_tx, mode_rx) = unbounded_channel::<ModeEvent>();
    let mode_handle = runtime.spawn(handle_mode(session.clone(), mode_tx.clone()));

    let (param_tx, param_rx) = unbounded_channel::<ParamEvent>();
    let param_handle = runtime.spawn(handle_params_changed(session.clone(), param_tx.clone()));

    let (comms_tx, comms_rx) = unbounded_channel::<CommsEvent>();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Achilles Telemetry",
        options,
        Box::new(move |_cc| {
            Ok(Box::<AchillesApp>::new(AchillesApp::new(
                live_rx,
                live_handle,
                mode_rx,
                mode_handle,
                mode_tx,
                param_rx,
                param_handle,
                param_tx,
                comms_rx,
                comms_tx,
                session,
                Arc::new(runtime),
            )))
        }),
    )
}

impl eframe::App for AchillesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.render_top_panel(ui);
        self.render_left_panel(ui);
        self.render_right_panel(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            self.render_graph(ui);
        });
    }

    fn on_exit(&mut self) {
        if let Some(h) = &self.live_handle {
            h.abort();
        }
        if let Some(h) = &self.mode_handle {
            h.abort();
        }
        if let Some(h) = &self.param_handle {
            h.abort();
        }
        self.viewer.close();
        if let Some(c) = &mut self.live_channel {
            c.close();
        }
        if let Some(c) = &mut self.mode_channel {
            c.close();
        }
        if let Some(c) = &mut self.param_channel {
            c.close();
        }
    }
}