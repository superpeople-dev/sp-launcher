//! The website's Ideas, Roadmap and Completed items, for the launcher's pages
//! of the same names: listing, comments, votes, new comments and new ideas.
//!
//! The webview may not reach outside hosts (CSP), so every call is made here,
//! with the player's session (auth.rs) as a bearer token -- the same session
//! the website itself uses, on the same routes (sp-website app/api/launcher,
//! app/api/roadmap). The website decides everything: who may vote, post,
//! comment; this side only carries requests and answers.

use serde::{Deserialize, Serialize};

use crate::auth::{client, send, site_url};
use crate::error::{LauncherError, Result};

// ---------------------------------------------------------------- types ---
// Mirrors src/types.ts (CommunityItem, CommunityComment) -- keep the two in step.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Person {
    pub name: String,
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tag {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: String,
    pub score: i64,
    #[serde(default)]
    pub my_vote: Option<String>,
    #[serde(default)]
    pub comment_count: u32,
    pub created_at: f64,
    #[serde(default)]
    pub completed_at: Option<f64>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub author: Option<Person>,
    /// Its type and platform tags, which an admin's edit keeps.
    #[serde(default)]
    pub type_id: Option<String>,
    #[serde(default)]
    pub platform_id: Option<String>,
}

/// A platform (Game, Launcher, ...) or a type (Bug, Idea, ...), by the website's id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

/// What a new idea is filed under, and an admin's edit sets.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Meta {
    pub platforms: Vec<Choice>,
    pub types: Vec<Choice>,
}

/// An admin, as the website names them: who an item is assigned to, and who
/// an admin can assign it to (the id only for admins who manage items).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Member {
    pub id: Option<String>,
    pub name: String,
    pub avatar: Option<String>,
}

/// An item's discussion, and what an admin sees of it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub comments: Vec<Comment>,
    /// Comments turned off by an admin: only admins may still post.
    pub off: bool,
    /// Whom the item is assigned to; none means the whole team.
    pub assignee: Option<Member>,
    /// Whom an admin who manages items can assign it to (empty for others).
    pub staff: Vec<Member>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub author: Option<Person>,
    /// Posted by the team (an admin on the website).
    pub official: bool,
    pub body: String,
    pub created_at: f64,
    /// Written by the player, who may delete it.
    pub mine: bool,
    pub replies: Vec<Comment>,
}

/// The player's vote after the website took it, and the score it made.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoteResult {
    pub score: i64,
    pub my_vote: Option<String>,
}

// ------------------------------------------------ the website's answers ---

#[derive(Deserialize)]
struct ItemsAnswer {
    items: Vec<Item>,
    #[serde(default)]
    platforms: Vec<Choice>,
    #[serde(default)]
    types: Vec<Choice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebAuthor {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    avatar: Option<String>,
    #[serde(default)]
    admin: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebComment {
    id: String,
    body: String,
    created_at: f64,
    #[serde(default)]
    author: Option<WebAuthor>,
    #[serde(default)]
    mine: bool,
    #[serde(default)]
    replies: Vec<WebComment>,
}

impl From<WebComment> for Comment {
    fn from(c: WebComment) -> Self {
        let official = c.author.as_ref().is_some_and(|a| a.admin);
        Comment {
            id: c.id,
            author: c.author.map(|a| Person { name: a.name, avatar: a.avatar.filter(|s| !s.is_empty()) }),
            official,
            body: c.body,
            created_at: c.created_at,
            mine: c.mine,
            replies: c.replies.into_iter().map(Comment::from).collect(),
        }
    }
}

#[derive(Deserialize)]
struct CommentsAnswer {
    #[serde(default)]
    comments: Vec<WebComment>,
    #[serde(default)]
    off: bool,
    #[serde(default)]
    assignee: Option<WebAuthor>,
    #[serde(default)]
    staff: Vec<WebAuthor>,
}

impl From<WebAuthor> for Member {
    fn from(a: WebAuthor) -> Self {
        Member { id: a.id, name: a.name, avatar: a.avatar.filter(|s| !s.is_empty()) }
    }
}

#[derive(Deserialize)]
struct CommentAnswer {
    comment: WebComment,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoteAnswer {
    vote_count: i64,
    #[serde(default)]
    voted: bool,
    #[serde(default)]
    downvoted: bool,
}

#[derive(Deserialize, Default)]
struct WebError {
    #[serde(default)]
    error: String,
}

/// The website's refusals (app/api/roadmap/*), as sentences.
pub fn explain(code: &str) -> String {
    match code {
        "banned" => "You are banned from posting on the website.".into(),
        "name" => "Your Discord name is not allowed on the website. Change it to post or vote.".into(),
        "limit" => "You already have 3 posts waiting for the team's review. Wait until they are reviewed.".into(),
        "invalid" => "That could not be posted. Check its length.".into(),
        "offensive" => "That text was refused. Please keep it friendly.".into(),
        "off" => "Comments are closed on this item.".into(),
        "forbidden" => "You do not have permission to do that.".into(),
        "missing" => "That comment no longer exists.".into(),
        _ => crate::auth::OOPS.into(),
    }
}

// ---------------------------------------------------------------- calls ---

async fn answer<T: for<'de> Deserialize<'de>>(res: reqwest::Response) -> Result<T> {
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if status.is_success() {
        return serde_json::from_str(&text).map_err(|_| LauncherError::Message(crate::auth::OOPS.into()));
    }
    if status.as_u16() == 401 {
        return Err(LauncherError::SignedOut);
    }
    let err: WebError = serde_json::from_str(&text).unwrap_or_default();
    Err(LauncherError::Message(explain(&err.error)))
}

fn with(request: reqwest::RequestBuilder, session: Option<&str>) -> reqwest::RequestBuilder {
    match session {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

/// A page's items ("ideas", "roadmap" or "completed") with the player's own
/// votes, and the platforms and types an idea can be filed under.
pub async fn items(session: Option<&str>, board: &str) -> Result<(Vec<Item>, Meta)> {
    if !matches!(board, "ideas" | "roadmap" | "completed") {
        return Err(LauncherError::Message(format!("Unknown page {board}.")));
    }
    let url = format!("{}/api/launcher/items?board={board}", site_url());
    let got: ItemsAnswer = answer(send(with(client()?.get(url), session)).await?).await?;
    Ok((got.items, Meta { platforms: got.platforms, types: got.types }))
}

pub async fn thread(session: Option<&str>, id: &str) -> Result<Thread> {
    let url = format!("{}/api/roadmap/comments", site_url());
    let request = client()?.get(url).query(&[("feedbackId", id)]);
    let got: CommentsAnswer = answer(send(with(request, session)).await?).await?;
    Ok(Thread {
        comments: got.comments.into_iter().map(Comment::from).collect(),
        off: got.off,
        assignee: got.assignee.map(Member::from),
        staff: got.staff.into_iter().map(Member::from).collect(),
    })
}

/// Presses an arrow, as on the website: the arrow already chosen takes the
/// vote back, the other one switches it.
pub async fn vote(session: &str, id: &str, direction: &str) -> Result<VoteResult> {
    if !matches!(direction, "up" | "down") {
        return Err(LauncherError::Message("A vote is up or down.".into()));
    }
    let url = format!("{}/api/roadmap/vote", site_url());
    let request = client()?.post(url).json(&serde_json::json!({ "feedbackId": id, "direction": direction }));
    let got: VoteAnswer = answer(send(with(request, Some(session))).await?).await?;
    let my_vote = if got.voted {
        Some("up".to_string())
    } else if got.downvoted {
        Some("down".to_string())
    } else {
        None
    };
    Ok(VoteResult { score: got.vote_count, my_vote })
}

pub async fn comment(session: &str, id: &str, body: &str) -> Result<Comment> {
    let url = format!("{}/api/roadmap/comments", site_url());
    let request = client()?.post(url).json(&serde_json::json!({ "feedbackId": id, "body": body }));
    let got: CommentAnswer = answer(send(with(request, Some(session))).await?).await?;
    Ok(got.comment.into())
}

/// Posts an idea ("idea") or a bug report ("bug"), filed under a platform id
/// or "other". It waits for the team's review before it shows.
pub async fn post_idea(session: &str, title: &str, description: &str, kind: &str, platform: &str) -> Result<()> {
    let kind = match kind {
        "bug" => "bug-report",
        _ => "feature-request",
    };
    let url = format!("{}/api/roadmap/ideas", site_url());
    let request = client()?.post(url).json(&serde_json::json!({
        "title": title,
        "description": description,
        "type": kind,
        "platform": platform,
    }));
    let _: serde_json::Value = answer(send(with(request, Some(session))).await?).await?;
    Ok(())
}

// ---------------------------------------------------------------- admin ---
// The website's own admin routes (app/api/admin/feedback, app/api/roadmap/
// comments), which check the admin's permissions on every call.

/// An admin change to an item: "status" (with status), "edit" (with title,
/// description, typeId, categoryId), "assign" (with assignee: an admin's id,
/// or null for the whole team) or "delete".
pub async fn admin(session: &str, id: &str, mut change: serde_json::Map<String, serde_json::Value>) -> Result<()> {
    const ALLOWED: [&str; 7] = ["action", "status", "title", "description", "typeId", "categoryId", "assignee"];
    change.retain(|key, _| ALLOWED.contains(&key.as_str()));
    if !matches!(change.get("action").and_then(|a| a.as_str()), Some("status" | "edit" | "assign" | "delete")) {
        return Err(LauncherError::Message("Unknown admin action.".into()));
    }
    change.insert("feedbackId".into(), id.into());
    let url = format!("{}/api/admin/feedback", site_url());
    let request = client()?.post(url).json(&change);
    let _: serde_json::Value = answer(send(with(request, Some(session))).await?).await?;
    Ok(())
}

/// Turns an item's comments off (only admins may post) or on again.
pub async fn comments_off(session: &str, id: &str, off: bool) -> Result<()> {
    let url = format!("{}/api/roadmap/comments", site_url());
    let request = client()?.patch(url).json(&serde_json::json!({ "feedbackId": id, "off": off }));
    let _: serde_json::Value = answer(send(with(request, Some(session))).await?).await?;
    Ok(())
}

/// Deletes a comment: the player's own, or any for an admin who moderates comments.
pub async fn delete_comment(session: &str, id: &str, comment_id: &str) -> Result<()> {
    let url = format!("{}/api/roadmap/comments", site_url());
    let request = client()?.delete(url).json(&serde_json::json!({ "feedbackId": id, "commentId": comment_id }));
    let _: serde_json::Value = answer(send(with(request, Some(session))).await?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_websites_items_read_as_the_launcher_draws_them() {
        let json = r##"{"items":[{"id":"a1","title":"Ninja class","description":"Bring it back","status":"open","score":14,
            "myVote":"up","commentCount":3,"createdAt":1790000000000,"completedAt":null,
            "tags":[{"name":"Idea","color":"#8fb0ff"}],"author":{"name":"Gaara","avatar":null}}],
            "platforms":[{"id":"p1","name":"Game"}]}"##;
        let got: ItemsAnswer = serde_json::from_str(json).unwrap();
        assert_eq!(got.items[0].my_vote.as_deref(), Some("up"));
        assert_eq!(got.items[0].score, 14);
        assert_eq!(got.platforms[0].name, "Game");
        // And back out in the frontend's camelCase.
        let out = serde_json::to_value(&got.items[0]).unwrap();
        assert_eq!(out["commentCount"], 3);
        assert_eq!(out["myVote"], "up");
    }

    #[test]
    fn a_team_comment_is_marked_official_and_replies_come_along() {
        let json = r#"{"comments":[{"id":"c1","body":"Fixed next update","createdAt":1,"author":{"name":"Alice","avatar":"","admin":true},
            "replies":[{"id":"r1","body":"Thanks","createdAt":2,"author":{"name":"Gaara"},"replies":[]}]}]}"#;
        let got: CommentsAnswer = serde_json::from_str(json).unwrap();
        assert!(!got.off && got.assignee.is_none() && got.staff.is_empty(), "a player's view has no admin parts");
        let comments: Vec<Comment> = got.comments.into_iter().map(Comment::from).collect();
        assert!(comments[0].official);
        assert_eq!(comments[0].author.as_ref().unwrap().avatar, None, "an empty picture is no picture");
        assert!(!comments[0].replies[0].official);
    }

    #[test]
    fn an_admin_sees_who_the_item_is_assigned_to_and_whom_to_assign_it_to() {
        let json = r#"{"comments":[],"off":true,"assignee":{"id":"42","name":"Alice","avatar":"a.png","admin":true},
            "staff":[{"id":"42","name":"Alice","admin":true},{"id":"43","name":"Riwoter","admin":true}]}"#;
        let got: CommentsAnswer = serde_json::from_str(json).unwrap();
        assert!(got.off);
        let assignee = Member::from(got.assignee.unwrap());
        assert_eq!(assignee.id.as_deref(), Some("42"));
        assert_eq!(got.staff.len(), 2);
    }

    #[test]
    fn refusals_become_sentences() {
        assert!(explain("limit").contains("3 posts"));
        assert!(explain("forbidden").contains("permission"));
        assert!(explain("offensive").contains("friendly"));
        assert!(explain("reflet").starts_with("Oops"));
    }
}
