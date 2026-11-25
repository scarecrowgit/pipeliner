//! "Write with AI": the request the model gets and the draft that comes back.

use llm::Reply;
use model::{ContentBlock, Role};

use super::Fx;
use crate::api::{ChatApi, DescriptionDraftRequest, IssueApi};

#[test]
fn a_draft_is_asked_from_title_note_and_images_and_comes_back_unfenced() {
    let fx = Fx::new();
    let image = fx.engine.put_image("crash.png", "image/png", b"PNG!").expect("image");
    fx.llm.push(Reply::text("```markdown\n").and_text("Login fails.\n\n### Done when\n\n- It works\n").and_text("```\n"));

    let draft = fx
        .engine
        .draft_description(DescriptionDraftRequest {
            title: "Login fails".into(),
            note: "after the token expires".into(),
            images: vec![image.clone()],
            repo: Some("payments".into()),
            previous: Some("Old draft".into()),
            instruction: Some("shorter".into()),
        })
        .expect("draft");
    assert_eq!(draft, "Login fails.\n\n### Done when\n\n- It works");

    let request = fx.llm.last_request().expect("a request");
    assert!(request.tools.is_empty());
    assert!(!request.system.is_empty());
    let [message] = request.messages.as_slice() else { panic!("one message") };
    assert_eq!(message.role, Role::User);
    let ContentBlock::Text { text } = &message.content[0] else { panic!("text first") };
    for wanted in ["Login fails", "after the token expires", "payments", "Old draft", "shorter", "crash.png"] {
        assert!(text.contains(wanted), "{wanted} missing in {text}");
    }
    assert!(matches!(&message.content[1], ContentBlock::Image { blob, name, .. } if *blob == image.blob && name == "crash.png"));
}

#[test]
fn nothing_to_work_from_is_refused_without_asking_the_model() {
    let fx = Fx::new();
    let request = DescriptionDraftRequest { title: "  ".into(), note: "\n".into(), ..Default::default() };
    let error = fx.engine.draft_description(request).expect_err("refused");
    assert!(error.to_string().contains("title or a note"), "{error}");
    assert_eq!(fx.llm.call_count(), 0);

    fx.llm.push(Reply::text("  \n"));
    let request = DescriptionDraftRequest { title: "A".into(), ..Default::default() };
    assert!(fx.engine.draft_description(request).is_err(), "an empty answer is an error");
}
