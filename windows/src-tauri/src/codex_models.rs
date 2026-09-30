//! Discover the installed CLI's model catalog over its local stdio protocol.

use std::collections::HashSet;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

const TIMEOUT: Duration = Duration::from_secs(20);
const CACHE_TTL: Duration = Duration::from_secs(300);
const MAX_OUTPUT: usize = 2_000_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffort {
    pub reasoning_effort: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub model: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub default_reasoning_effort: String,
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<ReasoningEffort>,
    #[serde(default, skip_serializing)]
    hidden: bool,
}

pub struct Selection {
    pub model: String,
    pub reasoning_effort: String,
}

#[derive(Default)]
pub struct Catalog(tokio::sync::Mutex<Option<(Instant, Vec<Model>)>>);

impl Catalog {
    pub async fn list(&self, refresh: bool) -> Result<Vec<Model>, String> {
        let mut cached = self.0.lock().await;
        if let Some((fetched, models)) = cached.as_ref() {
            if !refresh && fetched.elapsed() < CACHE_TTL {
                return Ok(models.clone());
            }
        }
        let models = discover().await?;
        *cached = Some((Instant::now(), models.clone()));
        Ok(models)
    }

    pub async fn resolve(&self, model: &str, effort: &str) -> Result<Selection, String> {
        select(&self.list(false).await?, model, effort)
    }
}

fn select(models: &[Model], requested: &str, effort: &str) -> Result<Selection, String> {
    let model = models
        .iter()
        .find(|model| {
            !model.hidden
                && if requested.is_empty() {
                    model.is_default
                } else {
                    model.model == requested
                }
        })
        .ok_or_else(|| {
            "This model is not in the Codex catalog. Open Settings and choose an available model."
                .to_string()
        })?;
    let effort = if effort.is_empty() {
        &model.default_reasoning_effort
    } else {
        effort
    };
    if !effort.is_empty()
        && !model
            .supported_reasoning_efforts
            .iter()
            .any(|e| e.reasoning_effort == effort)
    {
        return Err(format!(
            "{} does not support that reasoning effort. Choose a supported level in Settings.",
            model.display_name
        ));
    }
    Ok(Selection {
        model: model.model.clone(),
        reasoning_effort: effort.to_string(),
    })
}

async fn discover() -> Result<Vec<Model>, String> {
    let working_dir = crate::settings::local_dir().join("chat");
    std::fs::create_dir_all(&working_dir)
        .map_err(|e| format!("Cannot create the Codex working directory: {e}"))?;
    let mut child = crate::codex::cli()
        .current_dir(working_dir)
        .args([
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "model_provider=\"openai\"",
            "-c",
            "features.plugins=false",
            "-c",
            "features.apps=false",
            "-c",
            "features.hooks=false",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            format!("Cannot start the Codex model catalog. Check that Codex CLI is installed: {e}")
        })?;
    let mut input = child.stdin.take().ok_or("Cannot open Codex input")?;
    let mut output = BufReader::new(child.stdout.take().ok_or("Cannot open Codex output")?);
    let result = tokio::time::timeout(TIMEOUT, query(&mut output, &mut input)).await;
    // This helper never starts a conversation and must not remain as a daemon.
    let _ = child.kill().await;
    let _ = child.wait().await;
    result.map_err(|_| {
        "Loading Codex models timed out. Check your connection and retry.".to_string()
    })?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    data: Vec<Model>,
    next_cursor: Option<String>,
}

async fn query(
    reader: &mut (impl AsyncBufRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<Vec<Model>, String> {
    let mut budget = MAX_OUTPUT;
    write(writer, json!({
        "id": 1, "method": "initialize",
        "params": { "clientInfo": { "name": "coucou", "title": "Coucou", "version": env!("CARGO_PKG_VERSION") } }
    })).await?;
    response(reader, 1, &mut budget).await?;
    write(writer, json!({ "method": "initialized", "params": {} })).await?;
    let mut cursor: Option<String> = None;
    let mut cursors = HashSet::new();
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for id in 2..22 {
        write(
            writer,
            json!({
                "id": id, "method": "model/list",
                "params": { "limit": 100, "includeHidden": false, "cursor": cursor }
            }),
        )
        .await?;
        let page: Page =
            serde_json::from_value(response(reader, id, &mut budget).await?).map_err(|_| {
                "Codex returned an invalid model catalog. Update Codex CLI and retry.".to_string()
            })?;
        for model in page.data {
            if !model.hidden && !model.model.is_empty() && seen.insert(model.model.clone()) {
                models.push(model);
            }
        }
        match page.next_cursor {
            Some(next) if cursors.insert(next.clone()) => cursor = Some(next),
            Some(_) => {
                return Err(
                    "Codex repeated a model catalog page. Update Codex CLI and retry.".into(),
                )
            }
            None if models.is_empty() => {
                return Err("Codex returned no models. Check codex login status and retry.".into())
            }
            None => return Ok(models),
        }
    }
    Err("The Codex model catalog contains too many pages.".into())
}

async fn write(writer: &mut (impl AsyncWrite + Unpin), message: Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .map_err(|e| format!("Cannot write to Codex: {e}"))?;
    writer
        .flush()
        .await
        .map_err(|e| format!("Cannot flush Codex input: {e}"))
}

async fn response(
    reader: &mut (impl AsyncBufRead + Unpin),
    id: u64,
    budget: &mut usize,
) -> Result<Value, String> {
    loop {
        let mut line = Vec::new();
        let count = (&mut *reader)
            .take((*budget + 1) as u64)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|e| format!("Cannot read the Codex model catalog: {e}"))?;
        if count == 0 {
            return Err(
                "Codex closed the model catalog connection. Check codex login status and retry."
                    .into(),
            );
        }
        if count > *budget {
            return Err("The Codex model catalog exceeded its size limit.".into());
        }
        *budget -= count;
        let message: Value = serde_json::from_slice(&line)
            .map_err(|_| "Codex returned an invalid model catalog response.".to_string())?;
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if message.get("error").is_some() {
            return Err("Codex could not load its model catalog. Check codex login status, update Codex CLI, and retry.".into());
        }
        return message
            .get("result")
            .cloned()
            .ok_or_else(|| "Codex returned no model catalog result.".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str, default: bool, efforts: &[&str]) -> Model {
        Model {
            model: name.into(),
            display_name: name.into(),
            description: String::new(),
            is_default: default,
            default_reasoning_effort: efforts.first().copied().unwrap_or("").into(),
            supported_reasoning_efforts: efforts
                .iter()
                .map(|effort| ReasoningEffort {
                    reasoning_effort: (*effort).into(),
                    description: String::new(),
                })
                .collect(),
            hidden: false,
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn defaults_and_supported_efforts_are_model_specific() {
        let models = vec![
            model("small", false, &["low", "medium"]),
            model("large", true, &["high", "ultra"]),
        ];
        let automatic = select(&models, "", "").unwrap();
        assert_eq!(
            (
                automatic.model.as_str(),
                automatic.reasoning_effort.as_str()
            ),
            ("large", "high")
        );
        assert_eq!(
            select(&models, "small", "").unwrap().reasoning_effort,
            "low"
        );
        assert_eq!(
            select(&models, "large", "ultra").unwrap().reasoning_effort,
            "ultra"
        );
        assert!(select(&models, "small", "ultra").is_err());
        assert!(select(&models, "retired", "").is_err());
        assert!(select(&models, "large", "high\"\nfeatures.shell_tool=true").is_err());
        assert!(select(&[model("no-default", false, &[])], "", "").is_err());
        assert_eq!(
            select(&[model("no-reasoning", true, &[])], "", "")
                .unwrap()
                .reasoning_effort,
            ""
        );
    }

    #[test]
    fn discovery_handles_handshake_notifications_and_pagination() {
        runtime().block_on(async {
            let mut hidden = model("hidden", false, &["low"]);
            hidden.hidden = true;
            let mut hidden_json = serde_json::to_value(&hidden).unwrap();
            hidden_json["hidden"] = json!(true);
            let messages = [
                json!({"method":"notice"}), json!({"id":1,"result":{}}),
                json!({"id":2,"result":{"data":[model("small", true, &["low"])],"nextCursor":"next"}}),
                json!({"id":3,"result":{"data":[serde_json::to_value(model("small", true, &["low"])).unwrap(), hidden_json, serde_json::to_value(model("large", false, &["high"])).unwrap()],"nextCursor":null}}),
            ].map(|value| format!("{value}\n")).concat();
            let mut input = BufReader::new(messages.as_bytes());
            let mut requests = Vec::new();
            let models = query(&mut input, &mut requests).await.unwrap();
            assert_eq!(models.iter().map(|m| m.model.as_str()).collect::<Vec<_>>(), ["small", "large"]);
            let requests: Vec<Value> = String::from_utf8(requests).unwrap().lines().map(|s| serde_json::from_str(s).unwrap()).collect();
            assert_eq!(requests[0]["method"], "initialize");
            assert_eq!(requests[1]["method"], "initialized");
            assert_eq!(requests[3]["params"]["cursor"], "next");
        });
    }

    #[test]
    fn discovery_rejects_bad_or_unbounded_responses() {
        runtime().block_on(async {
            for data in [
                "",
                "not json\n",
                "{\"id\":1,\"error\":{}}\n",
                "{\"id\":1}\n",
            ] {
                let mut reader = BufReader::new(data.as_bytes());
                let mut budget = MAX_OUTPUT;
                assert!(response(&mut reader, 1, &mut budget).await.is_err());
            }
            let oversized = vec![b'x'; 65];
            let mut reader = BufReader::new(oversized.as_slice());
            let mut budget = 64;
            assert!(response(&mut reader, 1, &mut budget)
                .await
                .unwrap_err()
                .contains("size limit"));
            for pages in [
                vec![json!({"data":[],"nextCursor":null})],
                vec![
                    json!({"data":[],"nextCursor":"same"}),
                    json!({"data":[],"nextCursor":"same"}),
                ],
            ] {
                let mut messages = "{\"id\":1,\"result\":{}}\n".to_string();
                for (i, page) in pages.into_iter().enumerate() {
                    messages.push_str(&format!("{}\n", json!({"id":i+2,"result":page})));
                }
                assert!(
                    query(&mut BufReader::new(messages.as_bytes()), &mut Vec::new())
                        .await
                        .is_err()
                );
            }
        });
    }

    #[test]
    #[ignore = "requires an installed, signed-in Codex CLI"]
    fn installed_cli_returns_a_usable_catalog() {
        runtime().block_on(async {
            let models = discover().await.unwrap();
            let selection = select(&models, "", "").unwrap();
            println!(
                "{} models; default {} / {}",
                models.len(),
                selection.model,
                selection.reasoning_effort
            );
        });
    }
}
