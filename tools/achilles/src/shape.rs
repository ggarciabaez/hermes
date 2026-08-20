use eframe::egui::{
    epaint::{CircleShape, RectShape, TextShape},
    Color32, FontFamily, FontId, Galley, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2,
};
use egui_graphs::{DisplayEdge, DisplayNode, DrawContext, EdgeProps, Node, NodeProps};

use crate::model::{EdgeData, EdgeKind, EndpointKind, NodeData, NodeKind};

/// Custom node renderer for Achilles: domains are drawn as large rounded
/// squares, endpoints as small circles. Sizes come from the node payload kind,
/// so the two node types are visually separated at a glance.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AchillesNodeShape {
    pub pos: Pos2,
    pub selected: bool,
    pub dragged: bool,
    pub hovered: bool,
    pub color: Option<Color32>,
    pub label_text: String,
    pub kind: NodeKind,
    pub manifest_loaded: bool,
    /// half-extent for squares, radius for circles
    pub size: f32,
    /// Cached label galley: laid out once and reused every frame (layout is
    /// the dominant per-frame cost). Invalidated in `update()` when the label
    /// text changes; skipped by serde since `Arc<Galley>` isn't serializable.
    #[serde(skip)]
    label_galley: Option<std::sync::Arc<Galley>>,
}

fn size_for(kind: &NodeKind) -> f32 {
    match kind {
        NodeKind::Domain => 13.0,
        NodeKind::Endpoint(EndpointKind::Publisher) => 6.0,
        NodeKind::Endpoint(EndpointKind::Queryable) => 6.0,
    }
}

impl From<NodeProps<NodeData>> for AchillesNodeShape {
    fn from(props: NodeProps<NodeData>) -> Self {
        AchillesNodeShape {
            pos: props.location(),
            selected: props.selected,
            dragged: props.dragged,
            hovered: props.hovered,
            color: props.color(),
            label_text: props.label.clone(),
            kind: props.payload.kind.clone(),
            manifest_loaded: props.payload.manifest_loaded,
            size: size_for(&props.payload.kind),
            label_galley: None,
        }
    }
}

impl DisplayNode<NodeData, crate::model::EdgeData, petgraph::Directed, petgraph::stable_graph::DefaultIx>
    for AchillesNodeShape
{
    fn is_inside(&self, pos: Pos2) -> bool {
        match self.kind {
            NodeKind::Domain => {
                let d = (pos - self.pos).abs();
                d.x <= self.size && d.y <= self.size
            }
            NodeKind::Endpoint(_) => (pos - self.pos).length() <= self.size,
        }
    }

    fn closest_boundary_point(&self, dir: Vec2) -> Pos2 {
        let dir = if dir == Vec2::ZERO {
            Vec2::new(1.0, 0.0)
        } else {
            dir.normalized()
        };
        match self.kind {
            NodeKind::Domain => {
                let s = self.size / dir.x.abs().max(dir.y.abs());
                self.pos + dir * s
            }
            NodeKind::Endpoint(_) => self.pos + dir * self.size,
        }
    }

    fn shapes(&mut self, ctx: &DrawContext) -> Vec<Shape> {
        let mut res = Vec::with_capacity(2);
        let center = ctx.meta.canvas_to_screen_pos(self.pos);
        let size = ctx.meta.canvas_to_screen_size(self.size);
        let color = self.effective_color(ctx);
        let stroke = self.effective_stroke(ctx);

        match self.kind {
            NodeKind::Domain => {
                let rect = Rect::from_center_size(center, Vec2::splat(size * 2.0));
                res.push(RectShape::filled(rect, 4.0, color).into());
                res.push(RectShape::stroke(rect, 4.0, stroke, StrokeKind::Middle).into());
            }
            NodeKind::Endpoint(_) => {
                res.push(
                    CircleShape {
                        center,
                        radius: size,
                        fill: color,
                        stroke,
                    }
                    .into(),
                );
            }
        }

        if !(ctx.style.labels_always() || self.selected || self.dragged || self.hovered) {
            return res;
        }

        let galley = self.label_galley(ctx, color);
        res.push(Self::label_shape(galley, center, size, color));
        res
    }

    fn update(&mut self, state: &NodeProps<NodeData>) {
        self.pos = state.location();
        self.selected = state.selected;
        self.dragged = state.dragged;
        self.hovered = state.hovered;
        if state.label != self.label_text {
            self.label_text = state.label.clone();
            self.label_galley = None;
        }
        self.color = state.color();
        self.kind = state.payload.kind.clone();
        self.manifest_loaded = state.payload.manifest_loaded;
        self.size = size_for(&state.payload.kind);
    }
}

impl AchillesNodeShape {
    fn effective_color(&self, ctx: &DrawContext) -> Color32 {
        let style = if self.selected || self.dragged || self.hovered {
            ctx.ctx.global_style().visuals.widgets.active
        } else {
            ctx.ctx.global_style().visuals.widgets.inactive
        };
        let base = self.color.unwrap_or(style.fg_stroke.color);
        // A domain whose manifest hasn't been fetched yet renders dimmed.
        if matches!(self.kind, NodeKind::Domain) && !self.manifest_loaded {
            base.linear_multiply(0.35)
        } else {
            base
        }
    }

    fn effective_stroke(&self, ctx: &DrawContext) -> Stroke {
        let style = ctx.ctx.global_style();
        let stroke = ctx.style.resolve_node_stroke(
            self.selected,
            self.dragged,
            self.color,
            Stroke::default(),
            &style,
        );
        let mut s = stroke;
        s.width = 2.5;
        if self.selected {
            s.color = Color32::LIGHT_GREEN;
        } else if self.dragged {
            s.color = Color32::BLACK;
        } else {
            s.color = Color32::LIGHT_BLUE;
        }
        s
    }

    fn label_galley(&mut self, ctx: &DrawContext, color: Color32) -> std::sync::Arc<Galley> {
        if let Some(g) = &self.label_galley {
            return g.clone();
        }
        let font_size = match self.kind {
            NodeKind::Domain => 14.0,
            NodeKind::Endpoint(_) => 10.0,
        };
        let galley = ctx.ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                self.label_text.clone(),
                FontId::new(font_size, FontFamily::Monospace),
                color,
            )
        });
        self.label_galley = Some(galley.clone());
        galley
    }

    fn label_shape(
        galley: std::sync::Arc<Galley>,
        center: Pos2,
        size: f32,
        color: Color32,
    ) -> Shape {
        let label_pos = Pos2::new(center.x - galley.size().x / 2.0, center.y - size * 2.0);
        TextShape::new(label_pos, galley, color).into()
    }
}

type AchNode<'a> =
    Node<NodeData, EdgeData, petgraph::Directed, petgraph::stable_graph::DefaultIx, AchillesNodeShape>;

/// Custom edge renderer for Achilles: a straight line with a direction arrow.
///
/// Deliberately renders NO label and does no text layout — the default edge
/// shape laid out a galley via `fonts_mut` for every edge on every frame (the
/// dominant rendering cost once the graph has data edges). Owner edges
/// (domain -> endpoint) are gray, data edges (publisher -> subscribing
/// domain) are teal.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AchillesEdgeShape {
    pub selected: bool,
    pub order: usize,
    pub kind: EdgeKind,
}

impl From<EdgeProps<EdgeData>> for AchillesEdgeShape {
    fn from(p: EdgeProps<EdgeData>) -> Self {
        Self {
            selected: p.selected,
            order: p.order,
            kind: p.payload.kind.clone(),
        }
    }
}

impl DisplayEdge<NodeData, EdgeData, petgraph::Directed, petgraph::stable_graph::DefaultIx, AchillesNodeShape>
    for AchillesEdgeShape
{
    fn update(&mut self, state: &EdgeProps<EdgeData>) {
        self.selected = state.selected;
        self.order = state.order;
        self.kind = state.payload.kind.clone();
    }

    fn shapes(&mut self, start: &AchNode, end: &AchNode, ctx: &DrawContext) -> Vec<Shape> {
        let color = if self.selected {
            Color32::LIGHT_GREEN
        } else {
            match self.kind {
                EdgeKind::Owner => Color32::from_gray(150),
                EdgeKind::Data => Color32::from_rgb(90, 180, 180),
            }
        };
        let style = ctx.ctx.global_style();
        let stroke = ctx.style.resolve_edge_stroke(
            self.selected,
            self.order,
            Stroke::new(1.0, color),
            &style,
        );

        let dir = end.location() - start.location();
        let dir = if dir == Vec2::ZERO {
            Vec2::new(1.0, 0.0)
        } else {
            dir.normalized()
        };
        let a = ctx
            .meta
            .canvas_to_screen_pos(start.display().closest_boundary_point(dir));
        let b = ctx
            .meta
            .canvas_to_screen_pos(end.display().closest_boundary_point(-dir));

        let mut res = Vec::with_capacity(2);
        res.push(Shape::line(vec![a, b], stroke));
        if ctx.is_directed {
            let perp = Vec2::new(-dir.y, dir.x);
            let tip = b;
            let base = b - dir * 10.0;
            res.push(Shape::convex_polygon(
                vec![tip, base + perp * 4.0, base - perp * 4.0],
                stroke.color,
                Stroke::NONE,
            ));
        }
        res
    }

    fn is_inside(&self, start: &AchNode, end: &AchNode, pos: Pos2) -> bool {
        let dir = end.location() - start.location();
        let dir = if dir == Vec2::ZERO {
            Vec2::new(1.0, 0.0)
        } else {
            dir.normalized()
        };
        let a = start.display().closest_boundary_point(dir);
        let b = end.display().closest_boundary_point(-dir);
        let seg = b - a;
        let len_sq = seg.length_sq();
        if len_sq <= 1e-9 {
            return (pos - a).length() <= 8.0;
        }
        let t = ((pos - a).dot(seg) / len_sq).clamp(0.0, 1.0);
        let proj = a + seg * t;
        (pos - proj).length() <= 8.0
    }
}