//! Render exclusion uses a range and leaves the editable timeline intact.
use super::*;
use filmcraft_engine::edge_gaps::EdgeGaps;
use filmcraft_project::ItemId;
enum Action {
    Queue(Box<ReelInfo>, QueueAction),
    Command(String, serde_json::Value),
}
pub(super) struct Prompt {
    gaps: EdgeGaps,
    action: Action,
    project: std::sync::Arc<filmcraft_project::Project>,
}
pub(super) fn queue_guard(app: &mut FilmcraftApp, info: &ReelInfo, action: &QueueAction) -> bool {
    let Some(seq) = app.session.state.active_sequence else {
        return true;
    };
    let Some(gaps) = filmcraft_engine::edge_gaps::inspect(&app.session.project, seq) else {
        return true;
    };
    rt().gap_prompt = Some(Prompt { gaps, action: Action::Queue(Box::new(info.clone()), action.clone()), project: app.session.project.clone() });
    false
}
pub(super) fn command_guard(app: &mut FilmcraftApp, command: &str, params: &serde_json::Value) -> bool {
    let Some(seq) = params.get("sequence").and_then(|v| v.as_u64()).map(ItemId).or(app.session.state.active_sequence) else {
        return true;
    };
    let Some(mut gaps) = filmcraft_engine::edge_gaps::inspect(&app.session.project, seq) else {
        return true;
    };
    if let Ok(Some(range)) = filmcraft_engine::export_tools::range_param(&app.session, &app.session.project, seq, params.get("range"), params, command) {
        let head = range.start.0 < gaps.start;
        let tail = range.end().0 > gaps.end;
        if !head && !tail {
            return true;
        }
        gaps.head_seconds = if head { filmcraft_time::Tick(gaps.start.saturating_sub(range.start.0)).seconds() } else { 0.0 };
        gaps.tail_seconds = if tail { filmcraft_time::Tick(range.end().0.saturating_sub(gaps.end)).seconds() } else { 0.0 };
        gaps.start = gaps.start.max(range.start.0);
        gaps.end = gaps.end.min(range.end().0);
    }
    rt().gap_prompt = Some(Prompt { gaps, action: Action::Command(command.into(), params.clone()), project: app.session.project.clone() });
    false
}
pub(super) fn warning(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    if let Some(gaps) = app.session.state.active_sequence.and_then(|seq| filmcraft_engine::edge_gaps::inspect(&app.session.project, seq)) {
        let r = ui.label(
            RichText::new(format!(
                "Empty footage: {:.2}s at the beginning · {:.2}s at the end. Rendering will ask whether to exclude these edges.",
                gaps.head_seconds, gaps.tail_seconds
            ))
            .color(egui::Color32::from_rgb(245, 180, 70)),
        );
        app.auto.add_response("reel.edgeGaps.warning", &r, "Empty video edge warning");
    }
}
pub(super) fn overlay(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let summary = rt().gap_prompt.as_ref().map(|p| p.gaps);
    let Some(gaps) = summary else {
        return;
    };
    let mut choice = None;
    let mut open = true;
    egui::Window::new("Empty footage at video edges").id(egui::Id::new("reel-gap-warning")).collapsible(false).open(&mut open).default_width(450.0).show(
        ctx,
        |ui| {
            ui.label(format!("Beginning: {:.2} seconds without footage\nEnd: {:.2} seconds without footage", gaps.head_seconds, gaps.tail_seconds));
            ui.label("Backgrounds and text can hide these gaps. Excluding them changes this render's range and leaves the editable timeline intact.");
            for (value, id, label) in
                [(0, "exclude", "Exclude empty edges and continue"), (1, "keep", "Keep these edges and continue"), (2, "cancel", "Cancel render")]
            {
                let b = ui.add_enabled(value != 0 || gaps.start < gaps.end, egui::Button::new(label));
                app.auto.add_response(&format!("reel.edgeGaps.{id}"), &b, label);
                if b.clicked() {
                    choice = Some(value);
                }
            }
        },
    );
    if !open {
        choice = Some(2);
    }
    let Some(choice) = choice else {
        return;
    };
    let Some(prompt) = rt().gap_prompt.take() else {
        return;
    };
    if choice == 2 {
        return;
    }
    if !std::sync::Arc::ptr_eq(&prompt.project, &app.session.project) || app.session.state.active_sequence != Some(ItemId(gaps.sequence)) {
        app.ui.status = "The project changed. Start rendering again to check the new timeline.".into();
        return;
    }
    let range = (choice == 0).then_some((gaps.start, gaps.end));
    match prompt.action {
        Action::Queue(info, action) => enqueue_range(app, &info, action, range),
        Action::Command(command, mut params) => {
            if let Some((start, end)) = range {
                params["range"] = json!("custom");
                params["startTime"] = json!(start);
                params["endTime"] = json!(end);
            }
            app.ui.status = match app.session.execute(&command, params) {
                Ok(_) => "Render started with the selected range".into(),
                Err(e) => e.to_string(),
            };
        }
    }
}
