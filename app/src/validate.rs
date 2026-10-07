//! Pure validation of annotation segments.
//!
//! One validator serves both writers of segments: the annotations PUT (full
//! replacement, Task 11) and queue `complete` (stored segments must be sound
//! before a file is marked done). It is deliberately free of I/O so callers
//! own the loading strategy.

use serde::Deserialize;
use uuid::Uuid;

use crate::error::FieldError;
use crate::models::Label;

/// One segment as received over the wire; keys are camelCase per the API
/// contract (only `FieldError` and this DTO use camelCase — everything else
/// on the wire is snake_case).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentInput {
    pub label_id: Uuid,
    pub start_ms: i32,
    pub end_ms: i32,
}

/// Validate segments against a project palette:
///
/// - the label must belong to the project and must not be archived;
/// - `start_ms >= 0`;
/// - `end_ms > start_ms`;
/// - when the duration is known, `end_ms <= duration_ms`.
///
/// All violations are collected (not just the first); errors reference
/// segments by index through camelCase field paths like `segments[0].endMs`.
pub fn validate_segments(
    segs: &[SegmentInput],
    labels: &[Label],
    duration_ms: Option<i32>,
) -> Result<(), Vec<FieldError>> {
    let mut errors = Vec::new();
    for (index, seg) in segs.iter().enumerate() {
        match labels.iter().find(|label| label.id == seg.label_id) {
            None => errors.push(FieldError {
                field: format!("segments[{index}].labelId"),
                message: "label does not belong to this project".into(),
            }),
            Some(label) if label.archived => errors.push(FieldError {
                field: format!("segments[{index}].labelId"),
                message: "label is archived".into(),
            }),
            _ => {}
        }
        if seg.start_ms < 0 {
            errors.push(FieldError {
                field: format!("segments[{index}].startMs"),
                message: "start must be non-negative".into(),
            });
        }
        if seg.end_ms <= seg.start_ms {
            errors.push(FieldError {
                field: format!("segments[{index}].endMs"),
                message: "end must be greater than start".into(),
            });
        }
        if let Some(duration) = duration_ms {
            if seg.end_ms > duration {
                errors.push(FieldError {
                    field: format!("segments[{index}].endMs"),
                    message: "end must not exceed the file duration".into(),
                });
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(archived: bool) -> Label {
        Label {
            id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            name: "смех".into(),
            color: "#e6194b".into(),
            hotkey: None,
            sort_order: 0,
            archived,
        }
    }

    fn seg(label_id: Uuid, start_ms: i32, end_ms: i32) -> SegmentInput {
        SegmentInput {
            label_id,
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn valid_segments_pass() {
        let a = label(false);
        let b = label(false);
        let segs = [seg(a.id, 0, 100), seg(b.id, 100, 5_000)];

        assert!(validate_segments(&segs, &[a.clone(), b.clone()], Some(5_000)).is_ok());
        assert!(validate_segments(&segs, &[a, b], None).is_ok());
        assert!(validate_segments(&[], &[label(false)], Some(1_000)).is_ok());
    }

    #[test]
    fn unknown_label_rejected() {
        let a = label(false);
        let segs = [seg(Uuid::new_v4(), 0, 100)];

        let errors = validate_segments(&segs, &[a], None).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "segments[0].labelId");
        assert_eq!(errors[0].message, "label does not belong to this project");
    }

    #[test]
    fn archived_label_rejected() {
        let archived = label(true);
        let segs = [seg(archived.id, 0, 100)];

        let errors = validate_segments(&segs, &[archived], None).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "segments[0].labelId");
        assert_eq!(errors[0].message, "label is archived");
    }

    #[test]
    fn end_not_after_start_rejected() {
        let a = label(false);
        for (start, end) in [(100, 100), (200, 100)] {
            let segs = [seg(a.id, start, end)];
            let errors = validate_segments(&segs, std::slice::from_ref(&a), None).unwrap_err();
            assert_eq!(errors.len(), 1, "({start}, {end}): {errors:?}");
            assert_eq!(errors[0].field, "segments[0].endMs");
            assert_eq!(errors[0].message, "end must be greater than start");
        }
    }

    #[test]
    fn negative_start_rejected() {
        let a = label(false);
        let segs = [seg(a.id, -10, 100)];

        let errors = validate_segments(&segs, &[a], None).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "segments[0].startMs");
        assert_eq!(errors[0].message, "start must be non-negative");
    }

    #[test]
    fn end_beyond_duration_rejected() {
        let a = label(false);
        let segs = [seg(a.id, 0, 1_001)];

        let errors = validate_segments(&segs, std::slice::from_ref(&a), Some(1_000)).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "segments[0].endMs");
        assert_eq!(errors[0].message, "end must not exceed the file duration");

        assert!(validate_segments(&segs, &[a], None).is_ok());
    }

    #[test]
    fn multiple_errors_collected_in_order() {
        let a = label(false);
        let segs = [seg(Uuid::new_v4(), 0, 100), seg(a.id, 500, 100)];

        let errors = validate_segments(&segs, &[a], None).unwrap_err();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert_eq!(errors[0].field, "segments[0].labelId");
        assert_eq!(errors[1].field, "segments[1].endMs");
    }

    #[test]
    fn segment_input_deserializes_camel_case() {
        let id = Uuid::new_v4();
        let raw = format!("{{\"labelId\":\"{id}\",\"startMs\":100,\"endMs\":200}}");

        let parsed: SegmentInput = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.label_id, id);
        assert_eq!(parsed.start_ms, 100);
        assert_eq!(parsed.end_ms, 200);
    }
}
