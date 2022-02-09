use eframe::egui::{self, Color32};
use rr_data::{SpanId, Time};
use std::collections::{HashMap, HashSet};

const ERROR_COLOR: egui::Color32 = Color32::RED;

/// Running index about spans and events, and their structure.
#[derive(Default)]
pub struct SpanTree {
    callsites: HashMap<rr_data::CallsiteId, rr_data::Callsite>,
    nodes: HashMap<SpanId, SpanNode>,
    roots: HashSet<SpanId>,
    orphan_events: Vec<(Time, rr_data::DataEvent)>,
}

/// A span is created, and then is opened over many non-overlapping intervals.
#[derive(Debug)]
struct SpanNode {
    span: rr_data::Span,
    follows: Option<SpanId>,
    lifetime: TimeInterval,
    intervals: Vec<TimeInterval>,
    children: HashSet<SpanId>,
    events: Vec<(Time, rr_data::DataEvent)>,
}

#[derive(Debug, Default)]
struct TimeInterval {
    entered: Option<Time>,
    exited: Option<Time>,
}

impl std::fmt::Display for TimeInterval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn format_optional_time(time: Option<Time>) -> String {
            match time {
                Some(time) => crate::misc::format_time(&time),
                None => "?".to_owned(),
            }
        }

        write!(
            f,
            "[{} - {}]",
            format_optional_time(self.entered),
            format_optional_time(self.exited)
        )
    }
}

impl SpanTree {
    pub fn on_mesage(&mut self, message: &rr_data::Message) {
        let rr_data::Message { log_time, msg_enum } = message;
        match &msg_enum {
            rr_data::MessageEnum::NewCallsite(callsite) => {
                self.callsites.insert(callsite.id, callsite.clone());
            }
            rr_data::MessageEnum::NewSpan(span) => {
                let prev = self.nodes.insert(
                    span.id,
                    SpanNode {
                        span: span.clone(),
                        follows: None,
                        lifetime: TimeInterval {
                            entered: Some(*log_time),
                            exited: None,
                        },
                        children: Default::default(),
                        intervals: Default::default(),
                        events: Default::default(),
                    },
                );
                if prev.is_some() {
                    tracing::warn!("Reused span id");
                }

                if let Some(parent_span_id) = &span.parent_span_id {
                    if let Some(parent_node) = self.nodes.get_mut(parent_span_id) {
                        parent_node.children.insert(span.id);
                    } else {
                        tracing::warn!("Unknown parent span");
                    }
                } else {
                    self.roots.insert(span.id);
                }
            }

            rr_data::MessageEnum::EnterSpan(span_id) => {
                if let Some(node) = self.nodes.get_mut(span_id) {
                    node.intervals.push(TimeInterval {
                        entered: Some(*log_time),
                        exited: None,
                    });
                } else {
                    tracing::warn!("Opened unknown span");
                }
            }
            rr_data::MessageEnum::ExitSpan(span_id) => {
                if let Some(node) = self.nodes.get_mut(span_id) {
                    if let Some(interval) = node.intervals.last_mut() {
                        if interval.exited.is_none() {
                            interval.exited = Some(*log_time);
                        } else {
                            tracing::warn!("Exited span that was never opened");
                            node.intervals.push(TimeInterval {
                                entered: None,
                                exited: Some(*log_time),
                            });
                        }
                    } else {
                        tracing::warn!("Exited span that was never opened");
                        node.intervals.push(TimeInterval {
                            entered: None,
                            exited: Some(*log_time),
                        });
                    }
                } else {
                    tracing::warn!("Exited unknown span");
                }
            }
            rr_data::MessageEnum::DestroySpan(span_id) => {
                if let Some(node) = self.nodes.get_mut(span_id) {
                    if node.lifetime.exited.is_some() {
                        tracing::warn!("Destroying a span twice");
                    }
                    node.lifetime.exited = Some(*log_time);
                } else {
                    tracing::warn!("Destroying unknown span");
                }
            }
            rr_data::MessageEnum::SpanFollowsFrom { span, follows } => {
                if let Some(node) = self.nodes.get_mut(span) {
                    if node.follows.is_some() {
                        tracing::warn!("Span follows multiple spans");
                    }
                    node.follows = Some(*follows);
                } else {
                    tracing::warn!("Unknown span");
                }
            }
            rr_data::MessageEnum::DataEvent(event) => {
                if let Some(span_id) = &event.parent_span_id {
                    if let Some(node) = self.nodes.get_mut(span_id) {
                        node.events.push((*log_time, event.clone()));
                    } else {
                        tracing::warn!("Event with unknown span");
                    }
                } else {
                    self.orphan_events.push((*log_time, event.clone()));
                }
            }
        }
    }

    pub fn span_name(&self, span_id: &SpanId) -> String {
        if let Some(node) = self.nodes.get(span_id) {
            if let Some(callsite) = self.callsites.get(&node.span.callsite_id) {
                callsite.name.to_string()
            } else {
                span_id.to_string()
            }
        } else {
            span_id.to_string()
        }
    }

    pub fn span_ancestry(&self, span_id: &SpanId) -> String {
        let mut ancestry = vec![self.span_name(span_id)];

        let mut current = *span_id;

        while let Some(parent) = self
            .nodes
            .get(&current)
            .and_then(|node| node.span.parent_span_id)
        {
            ancestry.push(self.span_name(&parent));
            current = parent;
        }

        use itertools::Itertools as _;
        ancestry.iter().rev().join(" ➡ ")
    }
}

/// ## UI memebers:
impl SpanTree {
    pub fn tree_ui(&self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for span_id in &self.roots {
                    self.tree_node_ui(ui, 0, span_id);
                }

                if !self.orphan_events.is_empty() {
                    ui.separator();
                    ui.label("Events outside of any span:");
                    ui.indent("events", |ui| {
                        for (time, event) in &self.orphan_events {
                            self.ui_timed_data_event(ui, time, event);
                        }
                    });
                }
            });
    }

    fn tree_node_ui(&self, ui: &mut egui::Ui, depth: usize, span_id: &SpanId) {
        if let Some(node) = self.nodes.get(span_id) {
            let name = self.span_name(span_id);
            egui::CollapsingHeader::new(name)
                .id_source(span_id)
                .default_open(depth < 4)
                .show(ui, |ui| {
                    self.tree_node_ui_impl(ui, depth, node);
                });
        } else {
            ui.colored_label(ERROR_COLOR, "Missing span");
        }
    }

    fn tree_node_ui_impl(&self, ui: &mut egui::Ui, depth: usize, node: &SpanNode) {
        self.ui_span_summary(ui, node);
        for child in &node.children {
            self.tree_node_ui(ui, depth + 1, child);
        }
    }

    fn ui_timed_data_event(&self, ui: &mut egui::Ui, time: &Time, data_event: &rr_data::DataEvent) {
        let rr_data::DataEvent {
            callsite_id,
            parent_span_id: _,
            fields,
        } = data_event;

        let response = ui.horizontal(|ui| {
            let time = crate::misc::format_time(time);
            ui.label(egui::RichText::new(time).weak().monospace());

            for (key, value) in fields {
                ui.label(egui::RichText::new(format!("{}: ", key)).weak());
                ui.label(value.to_string());
            }
        });

        response.response.on_hover_ui(|ui| {
            ui.heading("Callsite:");
            self.callsite_ui_by_id(ui, callsite_id);
        });
    }

    pub fn callsite_ui_by_id(&self, ui: &mut egui::Ui, callsite_id: &rr_data::CallsiteId) {
        if let Some(callsite) = self.callsites.get(callsite_id) {
            crate::misc::ui_callsite(ui, callsite);
        } else {
            ui.label(format!("Unknown callsite: {}", callsite_id)); // error
        }
    }

    pub fn data_event_ui(&self, ui: &mut egui::Ui, data_event: &rr_data::DataEvent) {
        let rr_data::DataEvent {
            callsite_id,
            parent_span_id,
            fields,
        } = data_event;

        let response = ui.horizontal(|ui| {
            for (key, value) in fields {
                ui.label(egui::RichText::new(format!("{}: ", key)).weak());
                ui.label(value.to_string());
            }
        });

        response
            .response
            .on_hover_ui(|ui| {
                ui.heading("Callsite:");
                self.callsite_ui_by_id(ui, callsite_id);
            })
            .on_hover_ui(|ui| {
                ui.heading("Parent span:");
                if let Some(parent_span_id) = parent_span_id {
                    self.span_summary_ui_by_id(ui, parent_span_id);
                } else {
                    ui.label("<None>");
                }
            });
    }

    pub fn span_summary_ui_by_id(&self, ui: &mut egui::Ui, span_id: &rr_data::SpanId) {
        if let Some(node) = self.nodes.get(span_id) {
            self.ui_span_summary(ui, node);
        } else {
            ui.colored_label(ERROR_COLOR, "Missing span");
        }
    }

    fn ui_span_summary(&self, ui: &mut egui::Ui, node: &SpanNode) {
        let SpanNode {
            span,
            follows,
            lifetime,
            children: _,
            intervals,
            events,
        } = node;

        let rr_data::Span {
            id: _,
            parent_span_id,
            callsite_id,
        } = span;

        egui::Grid::new("span_node")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                if let Some(callsite) = self.callsites.get(callsite_id) {
                    // TODO: more?
                    ui.label("Name:");
                    ui.label(format!("{:?}", callsite.name.as_str()));
                    ui.end_row();

                    ui.label("Level:");
                    ui.label(callsite.level.to_string());
                    ui.end_row();

                    ui.label("Location:");
                    ui.label(callsite.location.to_string());
                    ui.end_row();
                } else {
                    ui.colored_label(ERROR_COLOR, "Missing callsite");
                    ui.end_row();
                }

                use itertools::Itertools as _;

                ui.label("Span ancestry:");
                if let Some(parent_span_id) = parent_span_id {
                    ui.label(format!(
                        "{} ➡ (this span)",
                        self.span_ancestry(parent_span_id)
                    ));
                } else {
                    ui.weak("(root)");
                }
                ui.end_row();

                ui.label("Follows span:");
                if let Some(follows) = follows {
                    ui.label(self.span_name(follows));
                }
                ui.end_row();

                ui.label("Lifetime:");
                ui.label(lifetime.to_string());
                ui.end_row();

                ui.label("Intervals:");
                ui.label(intervals.iter().join(", "));
                ui.end_row();

                ui.label("Events:");
                if events.is_empty() {
                    ui.weak("(none)");
                } else {
                    ui.vertical(|ui| {
                        for (time, event) in events {
                            self.ui_timed_data_event(ui, time, event);
                        }
                    });
                }
                ui.end_row();
            });
    }
}