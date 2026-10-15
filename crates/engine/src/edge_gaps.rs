//! Footage coverage at sequence edges, ignoring Reel graphics, logos and backdrop copies.
use filmcraft_project::{ItemId, Project};
use filmcraft_time::Tick;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeGaps {
    pub sequence: u64,
    pub start: i64,
    pub end: i64,
    pub duration: i64,
    pub head_seconds: f64,
    pub tail_seconds: f64,
}
pub fn inspect(project: &Project, sequence: ItemId) -> Option<EdgeGaps> {
    let q = project.sequence(sequence)?;
    let reel = crate::reel::for_sequence(project.reel.as_ref(), sequence);
    let footage_track = reel
        .as_ref()
        .and_then(|r| r.main_clip)
        .and_then(|id| q.video_tracks.iter().position(|t| t.items.iter().any(|c| c.id.0 == id)))
        .or_else(|| q.video_tracks.iter().position(|t| t.name == "Footage"));
    let mut first = None;
    let mut last = Tick::ZERO;
    for (index, track) in q.video_tracks.iter().enumerate().filter(|(_, t)| t.enabled) {
        if reel.is_some() && Some(index) != footage_track {
            continue;
        }
        for clip in track.items.iter().filter(|c| c.enabled && c.duration.0 > 0) {
            let video =
                project.resolve_media(clip.item).is_some_and(|(_, media, _)| media.info.has_video() && media.info.kind != filmcraft_media::MediaKind::Still)
                    || project.item(clip.item).is_some_and(|item| item.as_sequence().is_some());
            if !video {
                continue;
            }
            first = Some(first.map_or(clip.start, |old: Tick| old.min(clip.start)));
            last = last.max(clip.end());
        }
    }
    let first = first?.max(Tick::ZERO);
    let duration = q.duration();
    let last = last.min(duration);
    if first >= last {
        return None;
    }
    let tail = Tick(duration.0.saturating_sub(last.0));
    let frame = q.settings.frame_rate.frame_duration();
    if first < frame && tail < frame {
        return None;
    }
    Some(EdgeGaps { sequence: sequence.0, start: first.0, end: last.0, duration: duration.0, head_seconds: first.seconds(), tail_seconds: tail.seconds() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use serde_json::json;
    #[test]
    fn full_length_graphics_do_not_hide_empty_footage_edges() {
        let mut s = Session::default();
        s.execute("reel.new", json!({"template":"quoteCard","seconds":8})).unwrap();
        let media = s.execute("file.importDemoFootage", json!({"scene":"Aurora"})).unwrap()["items"][0].as_u64().unwrap();
        s.execute("reel.apply", json!({"item":media,"sourceIn":0,"sourceOut":8})).unwrap();
        let seq = s.state.active_sequence.unwrap();
        assert!(inspect(&s.project, seq).is_none());
        s.edit("Test gap", |p, _| {
            let q = p.sequence_mut(seq).unwrap();
            let clip = &mut q.video_tracks[2].items[0];
            clip.start = Tick::from_seconds_f64(1.0);
            clip.duration = Tick::from_seconds_f64(5.0);
            Ok(())
        })
        .unwrap();
        let gaps = inspect(&s.project, seq).unwrap();
        assert_eq!(gaps.head_seconds, 1.0);
        assert_eq!(gaps.tail_seconds, 2.0);
        s.edit("Reorder footage", |p, _| {
            p.sequence_mut(seq).unwrap().video_tracks.swap(1, 2);
            Ok(())
        })
        .unwrap();
        let reordered = inspect(&s.project, seq).unwrap();
        assert_eq!(reordered.head_seconds, 1.0);
        assert_eq!(reordered.tail_seconds, 2.0);
    }
}
