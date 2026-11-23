//! Live question decoding, validation and provider-specific response payloads.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// A live provider request. Answers are deliberately never persisted in SavedSession.
#[derive(Clone, Debug)]
pub struct QuestionRequest {
    pub id: Value,
    pub questions: Vec<Question>,
    pub title: Option<String>,
    pub(crate) claude_input: Option<Value>,
    pub(crate) source: QuestionSource,
    pub(crate) instance: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QuestionSource {
    ServerRequest,
    AsyncMessage,
}
#[derive(Clone, Debug)]
pub struct Question {
    pub id: String,
    pub header: String,
    pub text: String,
    pub options: Vec<QuestionOption>,
    pub multiple: bool,
    pub allow_custom: bool,
    pub secret: bool,
    pub kind: QuestionKind,
    pub placeholder: Option<String>,
    pub description: Option<String>,
}
#[derive(Clone, Debug)]
pub enum QuestionKind {
    Choice,
    Text,
    Number(QuestionNumber),
}
#[derive(Clone, Debug)]
pub struct QuestionNumber {
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
    pub default_value: Option<f64>,
    pub unit: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: Option<String>,
}
pub type QuestionAnswers = HashMap<String, Vec<String>>;
impl QuestionRequest {
    /// Local identity of this request, including when a provider ID is reused.
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// Async message questions remain live after their originating turn ends.
    pub fn turn_bound(&self) -> bool {
        self.source != QuestionSource::AsyncMessage
    }

    // Async questions are assistant-message metadata, not pending JSON-RPC requests.
    // Their answers enter the conversation as ordinary user input.
    pub(crate) fn from_async_message(item: &Value) -> Option<Self> {
        if item["type"] != "agentMessage"
            || item["delivery"] != "async"
            || item["groveQuestionResolved"] == true
        {
            return None;
        }
        let id = item["id"].as_str()?.to_owned();
        let rows = item["questions"].as_array()?;
        let normalized = rows.iter().enumerate().map(|(index, row)| {
            json!({"id":index.to_string(),"question":row["title"],"isOther":true,
                "options":row["options"].as_array().map(|options| options.iter().map(|label| json!({"label":label})).collect::<Vec<_>>())})
        }).collect::<Vec<_>>();
        let mut request = Self::parse(json!(id), &json!({"questions":normalized}), false).ok()?;
        request.source = QuestionSource::AsyncMessage;
        Some(request)
    }
    pub(crate) fn answer_text(&self, answers: &QuestionAnswers) -> String {
        self.questions
            .iter()
            .map(|question| format!("{}\n{}", question.text, answers[&question.id].join(", ")))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
    /// The ordinary user message emitted when answering an asynchronous question.
    /// Never retain secret answers for transcript deduplication.
    pub fn answer_echo_text(&self, answers: &QuestionAnswers) -> Option<String> {
        (self.source == QuestionSource::AsyncMessage
            && self.questions.iter().all(|question| !question.secret))
        .then(|| self.answer_text(answers))
    }
    /// Decode the app-server's structured input request without retaining answers.
    pub fn from_codex(id: Value, params: &Value) -> Result<Self, String> {
        Self::parse(id, params, false)
    }
    pub(crate) fn parse(id: Value, input: &Value, claude: bool) -> Result<Self, String> {
        if !(id.is_string() || id.is_number()) {
            return Err("Invalid question request identity".into());
        }
        let rows = input["questions"]
            .as_array()
            .filter(|rows| !rows.is_empty())
            .ok_or("The question request contains no questions")?;
        let mut questions = Vec::new();
        for row in rows {
            let text = row["question"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or("The question text is missing")?
                .to_owned();
            let key = if claude {
                Some(text.as_str())
            } else {
                row["id"].as_str()
            }
            .filter(|s| !s.trim().is_empty())
            .ok_or("The question identity is missing")?
            .to_owned();
            if questions.iter().any(|q: &Question| q.id == key) {
                return Err("The question request contains duplicate identities".into());
            }
            let options: Vec<QuestionOption> = if row["options"].is_null() {
                Vec::new()
            } else {
                serde_json::from_value(row["options"].clone())
                    .map_err(|_| "Invalid question choices")?
            };
            for (index, option) in options.iter().enumerate() {
                if option.label.trim().is_empty()
                    || options[..index].iter().any(|o| o.label == option.label)
                {
                    return Err("The question choices need unique nonempty labels".into());
                }
            }
            let kind = if claude {
                match row["kind"].as_str().unwrap_or("choice") {
                    "choice" => QuestionKind::Choice,
                    "text" => QuestionKind::Text,
                    "number" => {
                        let min = row["min"]
                            .as_f64()
                            .filter(|value| value.is_finite())
                            .ok_or("The numeric question minimum is missing")?;
                        let max = row["max"]
                            .as_f64()
                            .filter(|value| value.is_finite())
                            .ok_or("The numeric question maximum is missing")?;
                        let number_field = |key: &str| -> Result<Option<f64>, String> {
                            match row.get(key) {
                                None => Ok(None),
                                Some(value) => value
                                    .as_f64()
                                    .filter(|number| number.is_finite())
                                    .map(Some)
                                    .ok_or_else(|| {
                                        format!("The numeric question {key} must be a number")
                                    }),
                            }
                        };
                        let step = number_field("step")?;
                        let default_value = number_field("defaultValue")?;
                        if min >= max
                            || step.is_some_and(|value| !value.is_finite() || value <= 0.0)
                            || default_value.is_some_and(|value| {
                                !value.is_finite() || value < min || value > max
                            })
                        {
                            return Err("The numeric question constraints are invalid".into());
                        }
                        QuestionKind::Number(QuestionNumber {
                            min,
                            max,
                            step,
                            default_value,
                            unit: row["unit"].as_str().map(str::to_owned),
                        })
                    }
                    _ => return Err("Unsupported question kind".into()),
                }
            } else if options.is_empty() {
                QuestionKind::Text
            } else {
                QuestionKind::Choice
            };
            if !matches!(kind, QuestionKind::Choice)
                && (!options.is_empty() || row["multiSelect"] == true)
            {
                return Err("Text and number questions cannot have choices".into());
            }
            questions.push(Question {
                id: key,
                header: row["header"].as_str().unwrap_or_default().to_owned(),
                text,
                allow_custom: claude
                    || row["isOther"].as_bool().unwrap_or(false)
                    || options.is_empty(),
                multiple: claude && row["multiSelect"].as_bool().unwrap_or(false),
                secret: row["isSecret"].as_bool().unwrap_or(false),
                options,
                kind,
                placeholder: row["placeholder"].as_str().map(str::to_owned),
                description: row["description"].as_str().map(str::to_owned),
            });
        }
        Ok(Self {
            id,
            questions,
            title: input["title"].as_str().map(str::to_owned),
            claude_input: claude.then(|| input.clone()),
            source: QuestionSource::ServerRequest,
            instance: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
        })
    }
    pub fn validate(&self, answers: &QuestionAnswers) -> Result<(), String> {
        if answers.len() != self.questions.len() {
            return Err("Answer every question before submitting".into());
        }
        for question in &self.questions {
            let values = answers
                .get(&question.id)
                .ok_or("Answer every question before submitting")?;
            if values.is_empty()
                || (!question.multiple && values.len() != 1)
                || values.iter().enumerate().any(|(i, value)| {
                    value.trim().is_empty()
                        || values[..i].contains(value)
                        || (!question.allow_custom
                            && !question.options.iter().any(|o| o.label == *value))
                })
            {
                return Err("Choose or enter a valid answer for every question".into());
            }
            if let QuestionKind::Number(number) = &question.kind {
                let text = values[0].trim();
                let unsigned = text.strip_prefix('-').unwrap_or(text);
                let mut pieces = unsigned.split('.');
                let whole = pieces.next().unwrap_or_default();
                let fraction = pieces.next();
                let decimal = !whole.is_empty()
                    && whole.bytes().all(|byte| byte.is_ascii_digit())
                    && fraction.is_none_or(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
                    && pieces.next().is_none();
                let valid = decimal
                    && text.parse::<f64>().ok().is_some_and(|value| {
                        value.is_finite() && value >= number.min && value <= number.max
                    });
                if !valid {
                    return Err(format!(
                        "Enter a number between {} and {}",
                        number.min, number.max
                    ));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn codex_response(&self, answers: QuestionAnswers) -> Value {
        let answers: serde_json::Map<String, Value> = answers
            .into_iter()
            .map(|(key, answers)| (key, json!({"answers":answers})))
            .collect();
        json!({"answers":answers})
    }
    pub(crate) fn claude_response(&self, answers: QuestionAnswers) -> Value {
        let mut input = self.claude_input.clone().unwrap_or_else(|| json!({}));
        input["answers"] = answers
            .into_iter()
            .map(|(key, values)| (key, Value::String(values.join(", "))))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        json!({"behavior":"allow","updatedInput":input})
    }
}
/// Ephemeral successful answer delivery; deliberately contains no answer content.
#[derive(Clone, Debug)]
pub struct QuestionAnswerAck {
    pub sequence: u64,
    pub id: Value,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
}
