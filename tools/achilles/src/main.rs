mod liveliness;
use liveliness::*;
use std::collections::HashMap;
use eframe::egui;
use eframe::egui::Pos2;
use egui_graphs::{GraphView, Graph, SettingsInteraction, SettingsNavigation, GraphChange, SettingsStyle, GraphViewResponse};
use petgraph::graph::NodeIndex;

fn gen_force_view(interactions: &SettingsInteraction, styles: &SettingsStyle, navs: &SettingsNavigation) -> GraphView<egui_graphs::FruchtermanReingoldWithCenterGravityState, egui_graphs::LayoutForceDirected<egui_graphs::FruchtermanReingoldWithCenterGravity>>{
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

struct AchillesApp {
    selected_node_name: String,
    timeout_threshold: f32,
    is_live: bool,
    g: Graph,
    force: bool,
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
            force: false,
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
                            self.g.add_edge_with_label(*idx, ni, (), String::new());
                        }
                    }
                    self.node_map.insert(domain, ni);
                }
                LivelinessChange::Dropped(name) => {
                    let (_namespace, domain, _endpoint) = extract_domain(&name);
                    // These two don't need checks, as they just return None if the key doesn't exist.
                    if let Some(ni) = self.node_map.get(&domain) {
                        self.g.remove_node(*ni);
                        self.node_map.remove(&domain);
                    }


                }
            }
        }
        egui::Panel::top("top_panel").show(ui, |ui| {
            egui::MenuBar::default().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Exit").clicked() {
                        self.on_exit();
                    }
                })
            })
        });
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

                ui.toggle_value(&mut self.is_live, "Force Liveliness");
                ui.checkbox(&mut self.force, "Use Force on Graph");
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
        let styles = SettingsStyle::new()
            .with_labels_always(true) // Consider turning this to false if empty edge boxes appear
            .with_edge_stroke_hook(|_selected, _order, default_stroke, _egui_style| {
                // Create a custom thinner stroke for edges
                let mut stroke = default_stroke;
                stroke.width = 1.0; // Make edges thinner
                stroke.color = egui::Color32::from_gray(150).linear_multiply(0.5); // Softer color
                stroke
            })
            .with_node_stroke_hook(|_selected, _dragged, _node_color, current_stroke, _egui_style| {
                // Create a bolder, cleaner stroke for nodes
                let mut stroke = current_stroke;
                stroke.width = 2.5;
                if _selected {
                    stroke.color = egui::Color32::LIGHT_GREEN;
                } else if _dragged {
                    stroke.color = egui::Color32::BLACK;
                } else {
                    stroke.color = egui::Color32::LIGHT_BLUE; // Nicer node borders
                }
                stroke
            });
        egui::CentralPanel::default().show(ui, |ui| {
            let result: GraphViewResponse<petgraph::stable_graph::DefaultIx>;
            if self.force {
                result = gen_force_view(&interactions, &styles, &navs)
                    .show(ui, &mut self.g);
            } else {
                result = gen_default_view(&interactions, &styles, &navs)
                    .show(ui, &mut self.g);
            }
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