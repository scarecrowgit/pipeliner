//! "Write with AI": an issue description drafted by the default model from the title, the
//! person's note and the attached images. One request, no tools, nothing is stored.

use anyhow::{Result, anyhow, bail};
use base::cancel::CancelToken;
use llm::{BlobSource, ChatRequest, LlmError, Reasoning, StreamEvent};
use model::{BlobId, ContentBlock, Message, Role};

use crate::api::DescriptionDraftRequest;
use crate::engine::Engine;

/// Longest draft the model may write, in tokens.
const MAX_TOKENS: u32 = 1500;

const SYSTEM: &str = "You write issue descriptions for a coding agent and the team around it. \
Write a concise description in Markdown from the title, the person's note and any attached images.\n\
- Start with a summary of 1 to 3 sentences. No heading above it.\n\
- For a bug, add a \"### Steps to reproduce\" section with a numbered list.\n\
- Always end with a \"### Done when\" section: bullets with outcomes that can be checked.\n\
- Keep the person's facts. Do not invent specifics such as versions, numbers or file paths.\n\
- Refer to attached images by their file name.\n\
- No preamble and no closing remarks: answer with the description only.\n\
When a previous draft and an instruction are given, revise the previous draft as the instruction asks \
and answer with the whole new description. When only a previous draft is given, write a fresh alternative.";

/// Reads image bytes through the store.
struct StoreBlobs<'a>(&'a store::Store);

impl BlobSource for StoreBlobs<'_> {
    fn read(&self, id: &BlobId) -> std::io::Result<Vec<u8>> {
        self.0.read_blob(id).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{e:#}")))
    }
}

impl Engine {
    /// [`crate::api::IssueApi::draft_description`].
    pub(crate) fn write_description(&self, request: DescriptionDraftRequest) -> Result<String> {
        let inner = &self.inner;
        if request.title.trim().is_empty() && request.note.trim().is_empty() {
            bail!("Write a title or a note first, so there is something to work from");
        }
        let mut content = vec![ContentBlock::text(prompt(&request))];
        content.extend(request.images.iter().map(|image| ContentBlock::Image {
            blob: image.blob.clone(),
            media_type: image.media_type.clone(),
            name: image.name.clone(),
            bytes: image.bytes,
        }));
        let chat = ChatRequest {
            system: SYSTEM.to_string(),
            messages: vec![Message { role: Role::User, content }],
            tools: Vec::new(),
            max_tokens: MAX_TOKENS,
            temperature: None,
            reasoning: Reasoning::Off,
        };

        let model = inner.default_model();
        let mut text = String::new();
        inner
            .llm
            .stream(&model, &chat, &StoreBlobs(&inner.store), &CancelToken::new(), &mut |event| {
                if let StreamEvent::TextDelta(delta) = event {
                    text.push_str(&delta);
                }
            })
            .map_err(|e: LlmError| anyhow!("{e}"))?;

        let draft = strip_fence(&text);
        if draft.is_empty() {
            bail!("The model wrote nothing; try again");
        }
        Ok(draft)
    }
}

/// The user message's text: what the person gave, then the draft to revise.
fn prompt(request: &DescriptionDraftRequest) -> String {
    let mut out = String::new();
    let title = request.title.trim();
    out.push_str(&format!("Title: {}\n", if title.is_empty() { "(none yet)" } else { title }));
    if let Some(repo) = request.repo.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        out.push_str(&format!("Repo: {repo}\n"));
    }
    let note = request.note.trim();
    out.push_str(&format!("\nThe person's note:\n{}\n", if note.is_empty() { "(none)" } else { note }));
    if !request.images.is_empty() {
        let names: Vec<&str> = request.images.iter().map(|i| i.name.as_str()).collect();
        out.push_str(&format!("\nAttached images: {}\n", names.join(", ")));
    }
    if let Some(previous) = request.previous.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        out.push_str(&format!("\nPrevious draft:\n{previous}\n"));
        match request.instruction.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
            Some(instruction) => out.push_str(&format!("\nChange it as asked: {instruction}\n")),
            None => out.push_str("\nWrite a different version.\n"),
        }
    }
    out
}

/// The text without a wrapping Markdown code fence, trimmed.
fn strip_fence(text: &str) -> String {
    let text = text.trim();
    let Some(rest) = text.strip_prefix("```") else { return text.to_string() };
    let Some(rest) = rest.strip_suffix("```") else { return text.to_string() };
    // The first line is the fence's language tag (`markdown`, `md`, or nothing).
    let body = match rest.split_once('\n') {
        Some((tag, body)) if !tag.contains(' ') => body,
        _ => rest,
    };
    body.trim().to_string()
}
