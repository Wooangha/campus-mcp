//! The dashboard timeline: what is coming due.

use serde_json::Value;

use super::service::{service_data, string};
use crate::services::lms::{
    Activity, Deadline,
    plms::{PlmsError, page::text_of, sso::PlmsClient},
};
use crate::services::time::now_unix;

/// The service behind the dashboard's timeline block.
const ACTION_EVENTS: &str = "core_calendar_get_action_events_by_timesort";

impl PlmsClient {
    /// Deadlines still ahead, soonest first, as the dashboard timeline lists them.
    pub(super) async fn timeline(&self, limit: u32) -> Result<Vec<Deadline>, PlmsError> {
        let response = self
            .service(
                ACTION_EVENTS,
                serde_json::json!({
                    "limitnum": limit,
                    "timesortfrom": now_unix(),
                    "limittononsuspendedevents": true,
                }),
            )
            .await?;
        deadlines(&response)
    }
}

/// Reads the timeline service's response into deadlines, soonest first.
fn deadlines(json: &str) -> Result<Vec<Deadline>, PlmsError> {
    let data = service_data(json)?;
    let events = data["events"]
        .as_array()
        .ok_or(PlmsError::InvalidMoodleResponse)?;
    let mut deadlines: Vec<Deadline> = events
        .iter()
        .filter(|event| {
            // Assignment timelines can also contain expected-completion events
            // pointing at the same activity. Their timestamp is not its due date.
            // Older PLMS responses omit eventtype; retain that legacy behavior.
            event["modulename"].as_str() != Some("assign")
                || event
                    .get("eventtype")
                    .is_none_or(|kind| kind.as_str() == Some("due"))
        })
        .map(deadline)
        .collect();
    deadlines.sort_by_key(|deadline| deadline.due);
    Ok(deadlines)
}

fn deadline(event: &Value) -> Deadline {
    Deadline {
        name: string(&event["name"]),
        course: string(&event["course"]["fullname"]),
        kind: activity(event["modulename"].as_str().unwrap_or_default()),
        due: event["timesort"].as_i64().unwrap_or_default(),
        due_text: text_of(event["formattedtime"].as_str().unwrap_or_default()),
        url: string(&event["url"]),
        action: event["action"]["name"].as_str().map(str::to_owned),
    }
}

/// Moodle's activity module names, in the crate's own terms.
fn activity(module: &str) -> Activity {
    match module {
        "assign" => Activity::Assignment,
        "quiz" => Activity::Quiz,
        "vod" => Activity::Video,
        "attendance" => Activity::Attendance,
        "ubboard" => Activity::Board,
        other => Activity::Other(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped exactly like a live response, with the fields this module reads.
    const RESPONSE: &str = r#"[{"error":false,"data":{"events":[
        {"id":2,"name":"Project 1 is due","modulename":"assign","timesort":1790348340,
         "formattedtime":"<a href=\"/calendar/view.php?view=day&amp;time=1790348340\">2026년 9월 25일(금요일)</a>, 23:59",
         "url":"https://plms.postech.ac.kr/mod/assign/view.php?id=2",
         "course":{"fullname":"데이터베이스시스템"},
         "action":{"name":"과제 제출하기","actionable":true}},
        {"id":1,"name":"9/9 Video Lecture","modulename":"vod","timesort":1789657140,
         "formattedtime":"<a href=\"/calendar/view.php?view=day&amp;time=1789657140\">2026년 9월 17일(목요일)</a>, 23:59",
         "url":"https://plms.postech.ac.kr/mod/vod/view.php?id=1",
         "course":{"fullname":"컴파일러설계"},
         "action":{"name":"열람","actionable":true}}
    ],"firstid":1,"lastid":2}}]"#;

    #[test]
    fn deadlines_are_read_soonest_first() {
        let found = deadlines(RESPONSE).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "9/9 Video Lecture");
        assert_eq!(found[1].name, "Project 1 is due");
    }

    #[test]
    fn assignment_completion_events_do_not_replace_the_actual_due_date() {
        let mut response: Value = serde_json::from_str(RESPONSE).unwrap();
        let events = response[0]["data"]["events"].as_array_mut().unwrap();
        events[0]["eventtype"] = "due".into();
        let due = events[0].clone();
        let mut completion = due.clone();
        completion["eventtype"] = "expectcompletionon".into();
        completion["name"] = "Project 1 expected completion".into();
        completion["timesort"] = 1790434740_i64.into();
        events.push(completion.clone());
        let found = deadlines(&response.to_string()).unwrap();
        let assignments: Vec<_> = found.iter().filter(|item| item.is_assignment()).collect();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].due, 1790348340);
        assert_eq!(assignments[0].name, "Project 1 is due");
        assert_eq!(assignments[0].url, due["url"].as_str().unwrap());

        response[0]["data"]["events"] = serde_json::json!([completion]);
        assert!(deadlines(&response.to_string()).unwrap().is_empty());
    }

    #[test]
    fn legacy_assignments_and_other_activity_types_keep_their_existing_behavior() {
        let mut response: Value = serde_json::from_str(RESPONSE).unwrap();
        let events = response[0]["data"]["events"].as_array_mut().unwrap();
        events[1]["eventtype"] = "expectcompletionon".into();
        assert_eq!(deadlines(&response.to_string()).unwrap().len(), 2);

        for event_type in [
            serde_json::json!("opens"),
            Value::Null,
            serde_json::json!(42),
        ] {
            response[0]["data"]["events"][0]["eventtype"] = event_type;
            let found = deadlines(&response.to_string()).unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].kind, Activity::Video);
        }
    }

    #[test]
    fn a_deadline_carries_its_course_time_and_action() {
        let found = deadlines(RESPONSE).unwrap();
        let assignment = &found[1];
        assert_eq!(assignment.course, "데이터베이스시스템");
        assert_eq!(assignment.due_text, "2026년 9월 25일(금요일), 23:59");
        assert_eq!(assignment.action.as_deref(), Some("과제 제출하기"));
        assert_eq!(
            assignment.url,
            "https://plms.postech.ac.kr/mod/assign/view.php?id=2"
        );
    }

    #[test]
    fn moodle_modules_become_activities() {
        let found = deadlines(RESPONSE).unwrap();
        assert_eq!(found[0].kind, Activity::Video);
        assert_eq!(found[1].kind, Activity::Assignment);
        assert!(found[1].is_assignment());
        assert_eq!(activity("workshop"), Activity::Other("workshop".into()));
    }

    #[test]
    fn a_service_error_reaches_the_caller() {
        let json = r#"[{"error":true,"exception":{"message":"Invalid session key"}}]"#;
        assert!(matches!(
            deadlines(json).unwrap_err(),
            PlmsError::MoodleService(message) if message == "Invalid session key"
        ));
    }

    #[test]
    fn a_response_without_events_is_rejected() {
        let error = deadlines(r#"[{"error":false,"data":{}}]"#).unwrap_err();
        assert!(matches!(error, PlmsError::InvalidMoodleResponse));
    }
}
