//! Feed domain — read-only "recently added" content feeds from a media library
//! source (Plex today). A feed source is a provider like any other (registered
//! via `register_feed`, credentials encrypted at rest) but surfaces **content
//! items**, not controllable devices: nothing is persisted, every read is live
//! (behind a short server-side cache in `api::feeds`).

use serde::Serialize;

/// One library/section offered by a feed source (a Plex library) — the unit the
/// widget config picks from.
#[derive(Debug, Clone, Serialize)]
pub struct FeedLibrary {
    /// Provider-native library id (Plex section key).
    pub id: String,
    pub name: String,
    /// Provider-native kind (`movie` / `show` / `artist` / …), for the picker's
    /// glyph/label only — never behaviour.
    pub kind: String,
}

/// One item in a "recently added" feed, source-agnostic.
#[derive(Debug, Clone, Serialize)]
pub struct FeedItem {
    /// Provider-native item id (Plex rating key).
    pub id: String,
    /// Tile title — for an episode this is the **show** title (the tile wears
    /// the show poster; the episode itself goes in `subtitle`).
    pub title: String,
    /// Second line: `S2·E5` for an episode, the year for a movie, the artist
    /// for an album, ….
    pub subtitle: Option<String>,
    /// Item kind (`movie` / `episode` / `season` / `show` / `album` / `track`).
    pub kind: String,
    /// When the item was added, unix seconds — the feed's sort key and the
    /// widget's "2h ago" stamp.
    pub added_at: i64,
    /// Provider-relative poster path, served through the token-holding proxy
    /// (`GET /api/feeds/{id}/image?path=…`); never a raw upstream URL.
    pub image_path: Option<String>,
    /// Items sharing a key collapse into one tile ([`rollup`]) — a binge-import
    /// of one show must not flood every slot. Plex: the show's rating key on
    /// episodes/seasons. `None` = never grouped.
    pub group_key: Option<String>,
    /// Link that opens this item in the source's app (an `app.plex.tv` details
    /// URL) — sent to a TV remote as an app-link launch by the widget's
    /// optional tap action. For grouped kinds it targets the *show*, so a
    /// rolled-up tile needs no separate link.
    pub deep_link: Option<String>,
}

/// A feed tile after [`rollup`]: the representative (newest) item plus how many
/// raw items collapsed into it.
#[derive(Debug, Clone, Serialize)]
pub struct FeedEntry {
    #[serde(flatten)]
    pub item: FeedItem,
    /// Raw items behind this tile (1 = not grouped).
    pub count: usize,
}

/// What a rolled-up group's members are called ("3 new episodes").
fn group_noun(kind: &str) -> &'static str {
    match kind {
        "episode" => "episodes",
        "season" => "seasons",
        "track" => "tracks",
        "album" => "albums",
        _ => "items",
    }
}

/// Collapse raw feed items into at most `limit` tiles: items sharing a
/// `group_key` become one tile represented by the newest of them, its subtitle
/// replaced by the member count ("3 new episodes"). Runs in the shared api
/// layer — source-agnostic, so every future source inherits it. Items are
/// re-sorted newest-first, so provider ordering doesn't have to be trusted.
pub fn rollup(mut items: Vec<FeedItem>, limit: usize) -> Vec<FeedEntry> {
    items.sort_by_key(|i| std::cmp::Reverse(i.added_at));
    let mut entries: Vec<FeedEntry> = Vec::new();
    let mut group_index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for item in items {
        match &item.group_key {
            Some(key) => {
                if let Some(&idx) = group_index.get(key) {
                    entries[idx].count += 1;
                } else {
                    group_index.insert(key.clone(), entries.len());
                    entries.push(FeedEntry { item, count: 1 });
                }
            }
            None => entries.push(FeedEntry { item, count: 1 }),
        }
    }
    for e in &mut entries {
        if e.count > 1 {
            e.item.subtitle = Some(format!("{} new {}", e.count, group_noun(&e.item.kind)));
        }
    }
    entries.truncate(limit);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, added: i64, group: Option<&str>, kind: &str) -> FeedItem {
        FeedItem {
            id: id.into(),
            title: format!("title-{id}"),
            subtitle: Some(format!("sub-{id}")),
            kind: kind.into(),
            added_at: added,
            image_path: None,
            group_key: group.map(str::to_string),
            deep_link: None,
        }
    }

    #[test]
    fn rollup_collapses_a_group_onto_its_newest_item() {
        // Three episodes of one show + one movie, deliberately out of order.
        let entries = rollup(
            vec![
                item("e1", 100, Some("show-1"), "episode"),
                item("movie", 250, None, "movie"),
                item("e3", 300, Some("show-1"), "episode"),
                item("e2", 200, Some("show-1"), "episode"),
            ],
            10,
        );
        assert_eq!(entries.len(), 2);
        // Newest overall first; the group is represented by ITS newest (e3)
        // with the member count in the subtitle.
        assert_eq!(entries[0].item.id, "e3");
        assert_eq!(entries[0].count, 3);
        assert_eq!(entries[0].item.subtitle.as_deref(), Some("3 new episodes"));
        // The ungrouped movie keeps its own subtitle.
        assert_eq!(entries[1].item.id, "movie");
        assert_eq!(entries[1].count, 1);
        assert_eq!(entries[1].item.subtitle.as_deref(), Some("sub-movie"));
    }

    #[test]
    fn rollup_limit_applies_after_grouping_not_before() {
        // 5 episodes of one show + 2 movies: the group must count as ONE tile,
        // so a limit of 3 still shows both movies.
        let mut items: Vec<_> = (0..5)
            .map(|i| item(&format!("e{i}"), 100 + i, Some("s"), "episode"))
            .collect();
        items.push(item("m1", 500, None, "movie"));
        items.push(item("m2", 50, None, "movie"));
        let entries = rollup(items, 3);
        let ids: Vec<_> = entries.iter().map(|e| e.item.id.as_str()).collect();
        assert_eq!(ids, vec!["m1", "e4", "m2"]);
        assert_eq!(entries[1].count, 5);
    }

    #[test]
    fn rollup_distinct_groups_stay_distinct() {
        let entries = rollup(
            vec![
                item("a1", 10, Some("show-a"), "episode"),
                item("b1", 20, Some("show-b"), "episode"),
                item("a2", 30, Some("show-a"), "episode"),
            ],
            10,
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].item.id, "a2");
        assert_eq!(entries[0].count, 2);
        assert_eq!(entries[1].item.id, "b1");
        assert_eq!(entries[1].count, 1);
    }

    #[test]
    fn rollup_nouns_follow_the_item_kind() {
        let seasons = rollup(
            vec![
                item("s1", 1, Some("g"), "season"),
                item("s2", 2, Some("g"), "season"),
            ],
            5,
        );
        assert_eq!(seasons[0].item.subtitle.as_deref(), Some("2 new seasons"));
        let other = rollup(
            vec![
                item("x1", 1, Some("g"), "photo"),
                item("x2", 2, Some("g"), "photo"),
            ],
            5,
        );
        assert_eq!(other[0].item.subtitle.as_deref(), Some("2 new items"));
    }
}
