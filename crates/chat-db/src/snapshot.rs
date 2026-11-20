//! A viewer's whole live state, as a connection is first served it.

use std::collections::BTreeSet;

use chat_model::live::State;
use chat_model::{ChannelKind, Id};

use crate::{Db, Result};

impl Db {
    /// Everything `viewer` sees, apart from message history: their account,
    /// spaces with every channel in them (locked ones too — they show
    /// locked), their conversations, everyone in any of those, how they
    /// relate to people, presence, read markers, calls and rings.
    pub async fn snapshot(&self, viewer: Id) -> Result<State> {
        let account = self.account(viewer).await?;
        let mut state = State { me: Some(account.me.clone()), ..Default::default() };
        let mut people: BTreeSet<Id> = BTreeSet::from([viewer]);

        for space_id in self.spaces_of(viewer).await? {
            let space = self.space(space_id).await?;
            people.extend(space.members.keys().copied());
            for c in self.space_channels(space_id).await? {
                state.channels.insert(c.id, c);
            }
            state.space_order.push(space_id);
            state.spaces.insert(space_id, space);
        }
        for c in self.conversations_of(viewer).await? {
            people.extend(c.members.iter().copied());
            state.channels.insert(c.id, c);
        }
        for (other, relation) in self.relations_of(viewer).await? {
            people.insert(other);
            state.relations.insert(other, relation);
        }

        let ids: Vec<Id> = people.into_iter().collect();
        for u in self.users(&ids).await? {
            state.users.insert(u.id, u);
        }
        for (u, status) in self.presence(&ids).await? {
            if u != viewer {
                state.presence.insert(u, status);
            }
        }
        state.reads = self.reads_of(viewer).await?;
        // Markers for channels no longer visible are not the client's concern.
        state.reads.retain(|c, _| state.channels.contains_key(c));

        let call_channels: Vec<Id> =
            state.channels.values().filter(|c| c.kind.has_calls()).map(|c| c.id).collect();
        for v in self.voice_in(&call_channels).await? {
            state.voice.entry(v.channel).or_default().insert(v.user, v);
        }
        let conversations: Vec<Id> =
            state.channels.values().filter(|c| matches!(c.kind, ChannelKind::Direct | ChannelKind::Group)).map(|c| c.id).collect();
        for r in self.rings_in(&conversations).await? {
            state.rings.insert(r.channel, r);
        }
        state.muted = self.mutes_of(viewer).await?.into_iter().collect();
        state.settings = self
            .notification_settings(viewer, None)
            .await?
            .into_iter()
            .map(|s| (s.target, s))
            .collect();
        state.devices = self.devices(viewer).await?;
        Ok(state)
    }
}
