mod liveliness;
use liveliness::*;
use std::collections::HashMap;
use eframe::egui;
use eframe::egui::Pos2;
use egui_graphs::{GraphView, Graph, SettingsInteraction, SettingsNavigation, GraphChange, SettingsStyle};
use petgraph::graph::NodeIndex;

struct AchillesApp {
    selected_node_name: String,
    timeout_threshold: f32,
    is_live: bool,
    g: Graph,
    changes: Vec<GraphChange>,
    node_map: HashMap<String, NodeIndex>,
    live_channel: Option<UnboundedReceiver<LivelinessChange>>,
    live_handle: Option<tokio::task::JoinHandle<()>>,
}

impl AchillesApp {
    fn new(channel: UnboundedReceiver<LivelinessChange>, handle: tokio::task::JoinHandle<()>) -> Self {
        Self {
            live_channel: Some(channel),
            live_handle: Some(handle),
            ..Default::default()
        }
    }
}

impl Default for AchillesApp {
    fn default() -> Self {
        AchillesApp {
            selected_node_name: String::new(),
            timeout_threshold: 0.0,
            is_live: false,
            g: Graph::new(),  // empty graph
            changes: Vec::new(),
            node_map: HashMap::new(),
            live_channel: None,
            live_handle: None,
        }
    }
}

fn main() -> eframe::Result {
    let (tx, rx) = unbounded_channel::<LivelinessChange>();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .build()
        .expect("Failed to create runtime");
    let live_handle = runtime.spawn(handle_liveliness(tx));
    // 1. Configure the OS window (size, title, etc.)
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    // 2. Launch the application
    eframe::run_native(
        "Achilles Telemetry",
        options,
        // The closure `|cc|` gives you a CreationContext.
        // This is where you could load custom fonts or images on startup.
        Box::new(|_cc| {
            // Return our default app state wrapped in a Result and a Box
            Ok(Box::<AchillesApp>::new(AchillesApp::new(rx, live_handle)))
        }),
    )
}


impl eframe::App for AchillesApp {
    // This function runs ~60 times a second.
    // `ctx` (Context) is the egui engine. You use it to spawn windows and panels.

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(change) = self.live_channel.as_mut().unwrap().try_recv() {
            match change {
                LivelinessChange::Created { name, manifest } => {
                    println!("Received Manifest: {:?}", manifest);
                    let (_namespace, domain, _endpoint) = extract_domain(&name);
                    let ni = self.g.add_node_with_label_and_location((), domain.clone(), Pos2::new(rand::random(), rand::random()));
                    for idx in self.node_map.values(){
                        if *idx != ni {
                            self.g.add_edge(*idx, ni, ());
                        }
                    }
                    self.node_map.insert(domain, ni);
                }
                LivelinessChange::Dropped(name) => {
                    let (_namespace, domain, _endpoint) = extract_domain(&name);
                    let ni = self.node_map.get(&domain);
                    // These two don't need checks, as they just return None if the key doesn't exist.
                    self.g.remove_node(*ni.unwrap());
                    self.node_map.remove(&domain);

                }
            }
        }
        egui::Panel::left("inspector_panel")
            .resizable(true)
            .show(ui, |ui| {
                ui.heading("Node Inspector");
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.selected_node_name);
                });

                // A slider bound directly to our state variable
                ui.add(egui::Slider::new(&mut self.timeout_threshold, 0.1..=10.0).text("Timeout (s)"));

                // A checkbox bound to our boolean
                ui.checkbox(&mut self.is_live, "Force Liveliness");

                // Buttons return a boolean `true` on the exact frame they are clicked
                if ui.button("Reset Defaults").clicked() {
                    self.timeout_threshold = 1.0;
                    self.selected_node_name.clear();
                }
                ui.separator();
                ui.heading("Graph Response");

                if !self.changes.is_empty() {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for change in self.changes.iter() {
                            if let Some(nodeidx) = extract_node(change) {
                                ui.monospace(format!("{:?}: {change:?}", nodeidx));
                            }
                        }
                    });
                }

            });

        let interactions = SettingsInteraction::default()
            .with_node_clicking_enabled(true)
            .with_edge_clicking_enabled(true)
            .with_edge_selection_enabled(true)
            .with_node_selection_enabled(true);
        let navs = SettingsNavigation::default()
            .with_fit_to_screen_enabled(false)
            .with_zoom_and_pan_enabled(true);

        egui::CentralPanel::default().show(ui, |ui| {
            type L = egui_graphs::LayoutForceDirected<egui_graphs::FruchtermanReingoldWithCenterGravity>;
            type S = egui_graphs::FruchtermanReingoldWithCenterGravityState;
            //let result = DefaultGraphView::new()
            let result = GraphView::<S, L>::new()
                .with_interactions(&interactions)
                .with_styles(&SettingsStyle::new().with_labels_always(true))
                .with_navigations(&navs).show(ui, &mut self.g);
            self.changes = result.changes;
            if !self.changes.is_empty() {
                ;;
            }
        });
    }
    fn on_exit(&mut self) {
        self.live_handle.as_ref().unwrap().abort();
        self.live_channel.as_mut().unwrap().close();
    }
}