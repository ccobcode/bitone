//! 历史记录的分页.

use super::{HistoryItem, HistoryPage};
use crate::error::ApiError;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};

/// 游标里单个续页标记的长度上限.
const MAX_CURSOR_TOKEN: usize = 512;

/// 分页游标.一次查询里每个地址可能有多条独立分页的记录流,这里记下每条流的续页标记.
/// 键以地址在请求里的下标开头,流结束后从游标里删除.空表表示第一页.
pub type Cursor = BTreeMap<String, String>;

/// 校验客户端传回的游标.键和标记都可能被拼进上游请求,所以只放行固定的字符.
pub fn parse_cursor(text: &str, addresses: usize) -> Result<Cursor, ApiError> {
    let cursor: Cursor = serde_json::from_str(text).map_err(|_| ApiError::InvalidRequest)?;
    let safe = |text: &str| {
        text.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'='))
    };
    let valid = cursor.iter().all(|(key, token)| {
        stream_index(key).is_some_and(|index| index < addresses)
            && safe(key)
            && safe(token)
            && !token.is_empty()
            && token.len() <= MAX_CURSOR_TOKEN
    });
    if valid {
        Ok(cursor)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

pub fn stream_index(key: &str) -> Option<usize> {
    key.split(':').next()?.parse().ok()
}

/// 这一页要查的流:第一页是全部,之后只查游标里还有下文的.
pub fn streams(all: impl Iterator<Item = String>, cursor: &Cursor) -> Vec<(String, Option<String>)> {
    if cursor.is_empty() {
        all.map(|key| (key, None)).collect()
    } else {
        cursor
            .iter()
            .map(|(key, token)| (key.clone(), Some(token.clone())))
            .collect()
    }
}

/// 一条流的一页.
pub struct StreamPage {
    pub key: String,
    pub next: Option<String>,
    pub items: Vec<HistoryItem>,
}

/// 合并各条流.同一笔转账可能同时出现在转出方和转入方的流里.
pub fn merge_pages(pages: Vec<StreamPage>) -> HistoryPage {
    let mut seen = HashSet::new();
    let mut page = HistoryPage::default();
    for stream in pages {
        page.items
            .extend(stream.items.into_iter().filter(|item| seen.insert(item.clone())));
        if let Some(next) = stream.next {
            page.cursor.insert(stream.key, next);
        }
    }
    // 没有时间的是内存池里的交易,排在最前面.
    page.items.sort_by_key(|item| Reverse(item.time.unwrap_or(u64::MAX)));
    page
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstream::{NATIVE, TxStatus};

    #[test]
    fn cursor_only_accepts_known_streams_and_safe_tokens() {
        let good = parse_cursor(r#"{"0":"abc","1:native":"a_b-c=","1:tokentx":"7"}"#, 2).unwrap();
        assert_eq!(good.len(), 3);
        for bad in [
            "不是 JSON",
            "[]",
            r#"{"x":"abc"}"#,
            r#"{"2":"abc"}"#,
            r#"{"0":""}"#,
            r#"{"0":"../../admin"}"#,
            r#"{"0":"a&b=c"}"#,
            r#"{"0:/x":"a"}"#,
        ] {
            assert_eq!(parse_cursor(bad, 2).unwrap_err(), ApiError::InvalidRequest, "{bad}");
        }
        assert!(parse_cursor(&format!(r#"{{"0":"{}"}}"#, "a".repeat(MAX_CURSOR_TOKEN + 1)), 2).is_err());
    }

    #[test]
    fn pages_are_merged_newest_first_without_duplicates() {
        let item = |hash: &str, time: Option<u64>| HistoryItem {
            hash: hash.into(),
            time,
            from: None,
            to: None,
            asset: NATIVE.into(),
            amount: "1".into(),
            status: TxStatus::Included,
            flows: None,
        };
        let pages = vec![
            StreamPage {
                key: "0".into(),
                next: Some("n".into()),
                items: vec![item("a", Some(1)), item("b", Some(3))],
            },
            StreamPage {
                key: "1".into(),
                next: None,
                items: vec![item("b", Some(3)), item("c", None)],
            },
        ];
        let page = merge_pages(pages);
        let hashes: Vec<&str> = page.items.iter().map(|item| item.hash.as_str()).collect();
        assert_eq!(hashes, ["c", "b", "a"]);
        assert_eq!(page.cursor, Cursor::from([("0".to_string(), "n".to_string())]));
    }
}
