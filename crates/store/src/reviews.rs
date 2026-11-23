//! Reviews: the variants a pipeline hands to a person.

use anyhow::Result;
use model::{IssueKey, NodeId, Review, ReviewId, ReviewState, RunId};
use rusqlite::params;

use crate::Store;
use crate::codec::{Decoder, check_path, json, query_all, query_one, tag};

const REVIEW_COLUMNS: &str =
    "id, run, node, issue, title, state, created, agent, provenance, variants, renders, feedback, picked";

fn decode_review(d: &Decoder) -> Result<Review> {
    Ok(Review {
        id: ReviewId(d.string("id")?),
        run: RunId(d.string("run")?),
        node: NodeId(d.string("node")?),
        issue: d.opt::<IssueKey>("issue")?,
        title: d.string("title")?,
        state: d.tag("state")?,
        created: d.get("created")?,
        provenance: d.json("provenance")?,
        agent: d.string("agent")?,
        variants: d.json("variants")?,
        renders: d.json("renders")?,
        feedback: d.json("feedback")?,
        picked: d.opt_u32("picked")?,
    })
}

impl Store {
    /// Creates a review or replaces it.
    pub fn upsert_review(&self, review: &Review) -> Result<()> {
        for variant in &review.variants {
            if let Some(worktree) = &variant.worktree {
                check_path(worktree, "variant worktree")?;
            }
        }
        let values = (
            tag(&review.state)?,
            json(&review.provenance)?,
            json(&review.variants)?,
            json(&review.renders)?,
            json(&review.feedback)?,
        );
        self.write(|tx| {
            tx.prepare_cached(&format!(
                "INSERT INTO reviews ({REVIEW_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT (id) DO UPDATE SET run = excluded.run, node = excluded.node, issue = excluded.issue,
                    title = excluded.title, state = excluded.state, created = excluded.created,
                    agent = excluded.agent, provenance = excluded.provenance, variants = excluded.variants,
                    renders = excluded.renders, feedback = excluded.feedback, picked = excluded.picked"
            ))?
            .execute(params![
                review.id.as_str(),
                review.run.as_str(),
                review.node.as_str(),
                review.issue.as_ref().map(IssueKey::as_str),
                review.title,
                values.0,
                review.created,
                review.agent,
                values.1,
                values.2,
                values.3,
                values.4,
                review.picked,
            ])?;
            Ok(())
        })
    }

    /// One review.
    pub fn review(&self, id: &ReviewId) -> Result<Option<Review>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {REVIEW_COLUMNS} FROM reviews WHERE id = ?1"),
                [id.as_str()],
                "reviews",
                &["id"],
                decode_review,
            )
        })
    }

    /// Reviews still waiting for a person, oldest first.
    pub fn open_reviews(&self) -> Result<Vec<Review>> {
        let open = tag(&ReviewState::Open)?;
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {REVIEW_COLUMNS} FROM reviews WHERE state = ?1 ORDER BY created, id"),
                [open],
                "reviews",
                &["id"],
                decode_review,
            )
        })
    }

    /// Every review a run opened, oldest first.
    pub fn reviews_for_run(&self, run: &RunId) -> Result<Vec<Review>> {
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {REVIEW_COLUMNS} FROM reviews WHERE run = ?1 ORDER BY created, id"),
                [run.as_str()],
                "reviews",
                &["id"],
                decode_review,
            )
        })
    }
}
