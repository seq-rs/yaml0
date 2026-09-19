use std::ops::{Deref, DerefMut};

use serde::{Serialize, de::DeserializeOwned};

use crate::{Document, Result, SetOpts, edit::apply::Applied};

/// A struct wrapping a [`Document`], which can be mutated in place and committed back.
///
/// Derefs to `T`, so fields are assigned as usual. The values before and after are both kept, so
/// edits that e.g. set a value [`Some`] to [`None`] are recognized, and fields can removed (instead
/// of setting them to null).
///
/// ```
/// # use serde::{Deserialize, Serialize};
/// # use yaml0::Document;
/// #[derive(Serialize, Deserialize)]
/// struct Config {
///     image: Option<String>,
///     replicas: Option<u32>,
/// }
///
/// let mut doc = Document::new("image: nginx:1.25   # bump me\nreplicas: 2\n");
///
/// let mut cfg = doc.edit::<Config>()?;
/// cfg.image = Some("nginx:1.26".into());
/// cfg.commit()?;
///
/// assert_eq!(doc.as_str(), "image: nginx:1.26   # bump me\nreplicas: 2\n");
/// # Ok::<(), yaml0::Error>(())
/// ```
pub struct Edit<'d, T> {
    doc: &'d mut Document,
    before: T,
    after: T,
}

impl<T> Deref for Edit<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.after
    }
}

impl<T> DerefMut for Edit<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.after
    }
}

impl<T: Serialize> Edit<'_, T> {
    /// Apply changes to the inner [`Document`], with default [`SetOpts`] guarantees.
    pub fn commit(self) -> Result<Applied> {
        self.commit_with(SetOpts::default())
    }

    /// Apply changes to the inner [`Document`], just like [`Self::commit`], but with control over
    /// the guarantees (or behavior) of edits through [`opts`][`SetOpts`]
    pub fn commit_with(self, opts: SetOpts) -> Result<Applied> {
        let Edit { doc, before, after } = self;
        doc.apply_with(&before, &after, opts)
    }
}

impl Document {
    /// Parse this document into `T` for editing.
    ///
    /// Parses a copy to be mutated, and one to be used for diffs
    pub fn edit<T: DeserializeOwned + Serialize>(&mut self) -> Result<Edit<'_, T>> {
        let before = crate::from_str(&self.src)?;
        let after = crate::from_str(&self.src)?;
        Ok(Edit {
            doc: self,
            before,
            after,
        })
    }
}
