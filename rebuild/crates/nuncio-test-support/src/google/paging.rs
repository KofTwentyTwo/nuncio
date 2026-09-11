use super::{
    state::Model,
    wire::{Input, Reply, Result},
};
use serde_json::{json, Value};

pub(super) struct Page {
    pub scope: String,
    pub body: Value,
}
pub(super) fn scope(account: &str, input: &Input) -> String {
    let mut query = input.query.0.clone();
    query.remove("pageToken");
    format!("{account}|{}|{query:?}", input.path)
}
impl Model {
    pub fn saved_page(&self, scope: &str, input: &Input) -> Result<Option<Reply>> {
        match input.query.get("pageToken") {
            None => Ok(None),
            Some(token) => self
                .pages
                .get(token)
                .filter(|page| page.scope == scope)
                .map(|page| Some(Reply::json(page.body.clone())))
                .ok_or_else(|| Reply::error(400, "invalidArgument")),
        }
    }
    pub fn paginate(
        &mut self,
        scope: String,
        field: &str,
        items: Vec<Value>,
        size: usize,
        mut metadata: Value,
        final_fields: Value,
    ) -> Reply {
        let cap = size.min(self.page_cap).max(1);
        let step = if self.overlap {
            cap.saturating_sub(1).max(1)
        } else {
            cap
        };
        let pages = if items.len() <= cap {
            1
        } else {
            1 + (items.len() - cap).div_ceil(step)
        };
        let tokens: Vec<_> = (1..pages).map(|_| self.next("page")).collect();
        let mut first = Value::Null;
        for index in 0..pages {
            let start = index * step;
            metadata[field] =
                json!(&items[start.min(items.len())..((start + cap).min(items.len()))]);
            let mut body = metadata.clone();
            if let Some(next) = tokens.get(index) {
                body["nextPageToken"] = json!(next);
            } else if let (Some(body), Some(last)) =
                (body.as_object_mut(), final_fields.as_object())
            {
                body.extend(last.clone());
            }
            if index == 0 {
                first = body;
            } else if let Some(token) = tokens.get(index - 1) {
                self.pages.insert(
                    token.clone(),
                    Page {
                        scope: scope.clone(),
                        body,
                    },
                );
            }
        }
        Reply::json(first)
    }
}
