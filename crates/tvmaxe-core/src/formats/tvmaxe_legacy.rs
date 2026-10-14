//! The text format of the first TV-Maxe versions (the `subscriptions/` folder):
//!
//! ```text
//! |tvchannels||newchannel|Name|-|url|-|logo|-|epg_id|newchannel|…|endoftvchannels|
//! |radiochannels||newchannel|Name|-|url|-|logo|endofradiochannels|
//! ```

use super::non_empty;
use crate::{Channel, ChannelList, Kind, Logo};

pub fn parse_tvmaxe_legacy(data: &str) -> ChannelList {
    let mut list = ChannelList::default();
    for (kind, start, end) in [
        (Kind::Tv, "|tvchannels|", "|endoftvchannels|"),
        (Kind::Radio, "|radiochannels|", "|endofradiochannels|"),
    ] {
        // A file can have several sections of the same kind.
        for section in data.split(start).skip(1) {
            let body = section.split(end).next().unwrap_or("");
            for entry in body.split("|newchannel|").skip(1) {
                let f: Vec<&str> = entry.split("|-|").collect();
                let (Some(name), Some(url)) = (
                    f.first().copied().and_then(non_empty),
                    f.get(1).copied().and_then(non_empty),
                ) else {
                    continue;
                };
                list.channels.push(Channel {
                    kind,
                    name,
                    urls: vec![url],
                    group: None,
                    guide_id: f.get(3).copied().and_then(non_empty),
                    logo: f
                        .get(2)
                        .copied()
                        .and_then(non_empty)
                        .map_or(Logo::None, Logo::Url),
                    http: Default::default(),
                });
            }
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tv_and_radio_sections() {
        let src = "|tvchannels||newchannel|DR 1|-|rtmp://a/live1|-|http://i/1.png|-||newchannel|DR 2|-|rtmp://a/live2|-|http://i/2.png|-|dr2|endoftvchannels||radiochannels||newchannel|Radio X|-|http://r/x|-||endofradiochannels|";
        let l = parse_tvmaxe_legacy(src);
        assert_eq!(l.channels.len(), 3);
        assert_eq!(l.channels[0].name, "DR 1");
        assert_eq!(l.channels[0].logo, Logo::Url("http://i/1.png".into()));
        assert_eq!(l.channels[1].guide_id.as_deref(), Some("dr2"));
        assert_eq!(l.channels[2].kind, Kind::Radio);
        assert_eq!(l.channels[2].logo, Logo::None);
    }
}
