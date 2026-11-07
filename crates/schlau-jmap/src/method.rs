//! Generic standard methods (RFC 8620 section 5) parameterised by object type.

use std::collections::HashMap;
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::types::{Comparator, Filter, Id, IdsArg, PatchObject, ResultReference, SetError, State};

/// A JMAP data type such as `Email` or `Mailbox`.
pub trait JmapObject: DeserializeOwned {
    const TYPE: &'static str;
    /// Capabilities (besides core) a request must declare to use this type's methods.
    const USING: &'static [&'static str];
    type Condition: Serialize;
    type Create: Serialize;
    type GetArgs: Serialize + Default + Clone;
    type QueryArgs: Serialize + Default;
    type SetArgs: Serialize + Default;
}

/// A typed method call that can be added to a request.
pub trait Method: Serialize {
    type Response: DeserializeOwned;
    fn name(&self) -> String;
    fn using(&self) -> &'static [&'static str];
}

pub(crate) fn null_as_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Option::unwrap_or_default)
}

/// The `properties` argument: a literal list, `null` (all), or a back-reference.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum PropertiesArg {
    #[serde(rename = "properties")]
    List(Option<Vec<String>>),
    #[serde(rename = "#properties")]
    Ref(ResultReference),
}

impl Default for PropertiesArg {
    fn default() -> Self {
        Self::List(None)
    }
}

impl PropertiesArg {
    pub fn list<I, S>(props: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::List(Some(props.into_iter().map(Into::into).collect()))
    }
}

impl From<Option<Vec<String>>> for PropertiesArg {
    fn from(p: Option<Vec<String>>) -> Self {
        Self::List(p)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", bound(serialize = ""))]
pub struct Get<O: JmapObject> {
    pub account_id: Id,
    #[serde(flatten)]
    pub ids: IdsArg,
    #[serde(flatten)]
    pub properties: PropertiesArg,
    #[serde(flatten)]
    pub args: O::GetArgs,
    #[serde(skip)]
    _object: PhantomData<fn() -> O>,
}

impl<O: JmapObject> Get<O> {
    pub fn new(account_id: impl Into<Id>, ids: impl Into<IdsArg>) -> Self {
        Self {
            account_id: account_id.into(),
            ids: ids.into(),
            properties: PropertiesArg::default(),
            args: O::GetArgs::default(),
            _object: PhantomData,
        }
    }

    pub fn properties(mut self, properties: impl Into<PropertiesArg>) -> Self {
        self.properties = properties.into();
        self
    }

    pub fn args(mut self, args: O::GetArgs) -> Self {
        self.args = args;
        self
    }
}

impl<O: JmapObject> Method for Get<O> {
    type Response = GetResponse<O>;
    fn name(&self) -> String {
        format!("{}/get", O::TYPE)
    }
    fn using(&self) -> &'static [&'static str] {
        O::USING
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", bound = "O: DeserializeOwned")]
pub struct GetResponse<O> {
    pub account_id: Id,
    pub state: State,
    pub list: Vec<O>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub not_found: Vec<Id>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", bound(serialize = ""))]
pub struct Changes<O: JmapObject> {
    pub account_id: Id,
    pub since_state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_changes: Option<u64>,
    #[serde(skip)]
    _object: PhantomData<fn() -> O>,
}

impl<O: JmapObject> Changes<O> {
    pub fn new(account_id: impl Into<Id>, since_state: State, max_changes: Option<u64>) -> Self {
        Self {
            account_id: account_id.into(),
            since_state,
            max_changes,
            _object: PhantomData,
        }
    }
}

impl<O: JmapObject> Method for Changes<O> {
    type Response = ChangesResponse;
    fn name(&self) -> String {
        format!("{}/changes", O::TYPE)
    }
    fn using(&self) -> &'static [&'static str] {
        O::USING
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesResponse {
    pub account_id: Id,
    pub old_state: State,
    pub new_state: State,
    pub has_more_changes: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub created: Vec<Id>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub updated: Vec<Id>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub destroyed: Vec<Id>,
    /// `Mailbox/changes` only: when set, only these (count) properties changed for `updated`.
    #[serde(default)]
    pub updated_properties: Option<Vec<String>>,
}

impl ChangesResponse {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.updated.is_empty() && self.destroyed.is_empty()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", bound(serialize = ""))]
pub struct Query<O: JmapObject> {
    pub account_id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter<O::Condition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Comparator>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_offset: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calculate_total: Option<bool>,
    #[serde(flatten)]
    pub args: O::QueryArgs,
}

impl<O: JmapObject> Query<O> {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            filter: None,
            sort: None,
            position: None,
            anchor: None,
            anchor_offset: None,
            limit: None,
            calculate_total: None,
            args: O::QueryArgs::default(),
        }
    }

    pub fn filter(mut self, filter: impl Into<Filter<O::Condition>>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    pub fn sort(mut self, sort: impl IntoIterator<Item = Comparator>) -> Self {
        self.sort = Some(sort.into_iter().collect());
        self
    }

    pub fn position(mut self, position: i64) -> Self {
        self.position = Some(position);
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Id>, offset: i64) -> Self {
        self.anchor = Some(anchor.into());
        self.anchor_offset = Some(offset);
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn calculate_total(mut self, yes: bool) -> Self {
        self.calculate_total = Some(yes);
        self
    }

    pub fn args(mut self, args: O::QueryArgs) -> Self {
        self.args = args;
        self
    }
}

impl<O: JmapObject> Method for Query<O> {
    type Response = QueryResponse;
    fn name(&self) -> String {
        format!("{}/query", O::TYPE)
    }
    fn using(&self) -> &'static [&'static str] {
        O::USING
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    pub account_id: Id,
    pub query_state: State,
    pub can_calculate_changes: bool,
    pub position: u64,
    pub ids: Vec<Id>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", bound(serialize = ""))]
pub struct QueryChanges<O: JmapObject> {
    pub account_id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter<O::Condition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Comparator>>,
    pub since_query_state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_changes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub up_to_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calculate_total: Option<bool>,
    #[serde(flatten)]
    pub args: O::QueryArgs,
}

impl<O: JmapObject> QueryChanges<O> {
    /// Carries over filter, sort and extra arguments, which must match the original query.
    pub fn from_query(query: Query<O>, since_query_state: State) -> Self {
        Self {
            account_id: query.account_id,
            filter: query.filter,
            sort: query.sort,
            since_query_state,
            max_changes: None,
            up_to_id: None,
            calculate_total: query.calculate_total,
            args: query.args,
        }
    }

    pub fn max_changes(mut self, max: u64) -> Self {
        self.max_changes = Some(max);
        self
    }

    pub fn up_to_id(mut self, id: impl Into<Id>) -> Self {
        self.up_to_id = Some(id.into());
        self
    }
}

impl<O: JmapObject> Method for QueryChanges<O> {
    type Response = QueryChangesResponse;
    fn name(&self) -> String {
        format!("{}/queryChanges", O::TYPE)
    }
    fn using(&self) -> &'static [&'static str] {
        O::USING
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AddedItem {
    pub id: Id,
    pub index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryChangesResponse {
    pub account_id: Id,
    pub old_query_state: State,
    pub new_query_state: State,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub removed: Vec<Id>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub added: Vec<AddedItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", bound(serialize = ""))]
pub struct Set<O: JmapObject> {
    pub account_id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<State>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub create: HashMap<String, O::Create>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub update: HashMap<Id, PatchObject>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub destroy: Vec<Id>,
    #[serde(flatten)]
    pub args: O::SetArgs,
}

impl<O: JmapObject> Set<O> {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            if_in_state: None,
            create: HashMap::new(),
            update: HashMap::new(),
            destroy: Vec::new(),
            args: O::SetArgs::default(),
        }
    }

    pub fn if_in_state(mut self, state: State) -> Self {
        self.if_in_state = Some(state);
        self
    }

    pub fn create(mut self, creation_id: impl Into<String>, object: O::Create) -> Self {
        self.create.insert(creation_id.into(), object);
        self
    }

    pub fn update(mut self, id: impl Into<Id>, patch: PatchObject) -> Self {
        self.update.insert(id.into(), patch);
        self
    }

    /// Apply the same patch to every id.
    pub fn update_all<I, T>(self, ids: I, patch: &PatchObject) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        ids.into_iter()
            .fold(self, |set, id| set.update(id, patch.clone()))
    }

    pub fn destroy<I, T>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        self.destroy.extend(ids.into_iter().map(Into::into));
        self
    }

    pub fn args(mut self, args: O::SetArgs) -> Self {
        self.args = args;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.create.is_empty() && self.update.is_empty() && self.destroy.is_empty()
    }
}

impl<O: JmapObject> Method for Set<O> {
    type Response = SetResponse<O>;
    fn name(&self) -> String {
        format!("{}/set", O::TYPE)
    }
    fn using(&self) -> &'static [&'static str] {
        O::USING
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", bound = "O: DeserializeOwned")]
pub struct SetResponse<O> {
    pub account_id: Id,
    #[serde(default)]
    pub old_state: Option<State>,
    pub new_state: State,
    /// Creation id to the server-set properties of the new object (always includes `id`).
    #[serde(default = "HashMap::new", deserialize_with = "null_as_default")]
    pub created: HashMap<String, O>,
    /// Id to the server-changed properties beyond those in the patch, if any.
    #[serde(default, deserialize_with = "null_as_default")]
    pub updated: HashMap<Id, Option<Value>>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub destroyed: Vec<Id>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub not_created: HashMap<String, SetError>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub not_updated: HashMap<Id, SetError>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub not_destroyed: HashMap<Id, SetError>,
}

impl<O> SetResponse<O> {
    pub fn is_success(&self) -> bool {
        self.not_created.is_empty() && self.not_updated.is_empty() && self.not_destroyed.is_empty()
    }

    /// Merge a later chunk's results into this one; `new_state` becomes the later one.
    pub fn merge(mut self, other: SetResponse<O>) -> Self {
        self.new_state = other.new_state;
        self.created.extend(other.created);
        self.updated.extend(other.updated);
        self.destroyed.extend(other.destroyed);
        self.not_created.extend(other.not_created);
        self.not_updated.extend(other.not_updated);
        self.not_destroyed.extend(other.not_destroyed);
        self
    }
}
