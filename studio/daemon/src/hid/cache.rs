//! HID descriptors are immutable while their device path remains present.
//! Upstream openrazer-win #15: never cache a partial capability read, and prune
//! paths on unplug so a replacement is described again.
use super::HidInfo;
use crate::Result;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(crate) struct DescriptionCache(BTreeMap<String, HidInfo>);

impl DescriptionCache {
    pub fn get_or_describe(
        &mut self,
        path: &str,
        describe: impl FnOnce() -> Result<(HidInfo, bool)>,
    ) -> Result<HidInfo> {
        if let Some(info) = self.0.get(path) {
            return Ok(info.clone());
        }
        let (info, complete) = describe()?;
        if complete {
            self.0.insert(path.into(), info.clone());
        }
        Ok(info)
    }

    pub fn retain_present(&mut self, present: &BTreeSet<String>) {
        self.0.retain(|path, _| present.contains(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_paths_are_reused_until_unplugged() {
        let mut cache = DescriptionCache::default();
        let info = HidInfo {
            path: "device".into(),
            feature_length: 91,
            ..Default::default()
        };
        cache
            .get_or_describe("device", || Ok((info.clone(), true)))
            .unwrap();
        cache.retain_present(&BTreeSet::from(["device".into()]));
        assert_eq!(
            cache
                .get_or_describe("device", || panic!("unnecessary HID open"))
                .unwrap(),
            info
        );
        cache.retain_present(&BTreeSet::new());
        let replacement = HidInfo {
            product_id: 42,
            ..info
        };
        assert_eq!(
            cache
                .get_or_describe("device", || Ok((replacement.clone(), true)))
                .unwrap(),
            replacement
        );
    }

    #[test]
    fn partial_capabilities_and_failures_are_retried() {
        let mut cache = DescriptionCache::default();
        let info = HidInfo::default();
        cache
            .get_or_describe("device", || Ok((info.clone(), false)))
            .unwrap();
        assert!(cache
            .get_or_describe("device", || Err(crate::Error::Io("busy".into())))
            .is_err());
        let complete = HidInfo {
            feature_length: 91,
            ..info
        };
        assert_eq!(
            cache
                .get_or_describe("device", || Ok((complete.clone(), true)))
                .unwrap(),
            complete
        );
    }
}
