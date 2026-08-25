use revision::revisioned;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{
    Error, Result,
    component::{Component, ValidationContext},
};

use super::Origin;

#[revisioned(revision = 1)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct Group {
    pub origin: Origin,
    pub name: String,
}

impl Component for Group {
    const KIND: &'static str = "dev.koharu.group";

    fn validate(&self, _context: &ValidationContext<'_>) -> Result<()> {
        self.origin.validate()?;
        if self.name.len() <= 4096 && !self.name.contains('\0') {
            Ok(())
        } else {
            Err(Error::invalid("group name is invalid"))
        }
    }

    fn origin(&self) -> Option<&Origin> {
        Some(&self.origin)
    }

    fn set_origin(&mut self, origin: Origin) -> bool {
        self.origin = origin;
        true
    }
}

#[revisioned(revision = 1)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TextGroupKind {
    Main,
    Sfx,
}

/// Identifies a page-owned text group and the ordering responsibility it owns.
#[revisioned(revision = 1)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
pub struct TextGroup {
    pub kind: TextGroupKind,
}

impl Component for TextGroup {
    const KIND: &'static str = "dev.koharu.text.group";
}
