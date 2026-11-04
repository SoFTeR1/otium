//! The OTIUM account stored under `social` in library.json, in the Electron build's format
//! (fields this build doesn't know, like groups, are kept untouched).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Friend {
    pub pk: String,
    #[serde(default)]
    pub name: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Request {
    pub pk: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    pub from: String,
    pub text: String,
    pub at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub sk: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub friends: Vec<Friend>,
    #[serde(default)]
    pub requests: Vec<Request>,
    #[serde(default)]
    pub outgoing: Vec<String>,
    #[serde(default)]
    pub last_read: BTreeMap<String, i64>,
    #[serde(default)]
    pub history: BTreeMap<String, Vec<Message>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Account {
    pub fn load(v: Option<&Value>) -> Option<Self> {
        let a: Account = serde_json::from_value(v?.clone()).ok()?;
        (!a.sk.is_empty()).then_some(a)
    }

    pub fn is_friend(&self, pk: &str) -> bool {
        self.friends.iter().any(|f| f.pk == pk)
    }

    pub fn name_of(&self, pk: &str) -> Option<&str> {
        self.friends
            .iter()
            .find(|f| f.pk == pk)
            .map(|f| f.name.as_str())
            .or_else(|| self.requests.iter().find(|r| r.pk == pk).map(|r| r.name.as_str()))
            .filter(|n| !n.is_empty())
    }

    pub fn unread(&self, pk: &str, me: &str) -> usize {
        let last = self.last_read.get(pk).copied().unwrap_or(0);
        self.history.get(pk).map_or(0, |h| h.iter().filter(|m| m.from != me && m.at > last).count())
    }

    /// Adds a message to a chat; false if it was already there.
    pub fn push_message(&mut self, other: &str, m: Message) -> bool {
        let list = self.history.entry(other.to_string()).or_default();
        if list.iter().any(|x| x.id == m.id) {
            return false;
        }
        list.push(m);
        list.sort_by_key(|x| x.at);
        let extra = list.len().saturating_sub(300);
        list.drain(..extra);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unknown_fields_and_numbers() {
        let v: Value = serde_json::json!({
            "sk": "ab", "name": "X",
            "friends": [{ "pk": "p1", "name": "F", "note": 1 }],
            "requests": [], "outgoing": [],
            "lastRead": { "p1": 1793799070809i64, "g:1": 5 },
            "history": { "p1": [{ "id": "i", "from": "p1", "text": "hi", "at": 1793747208000i64 }] },
            "groups": [{ "id": "g" }]
        });
        let a = Account::load(Some(&v)).unwrap();
        assert_eq!(serde_json::to_value(&a).unwrap(), v);
    }

    #[test]
    fn dedupes_messages() {
        let mut a = Account { sk: "x".into(), ..Default::default() };
        let m = Message { id: "1".into(), from: "a".into(), text: "t".into(), at: 1 };
        assert!(a.push_message("a", m.clone()));
        assert!(!a.push_message("a", m));
    }
}
