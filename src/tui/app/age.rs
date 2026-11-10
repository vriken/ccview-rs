use crate::{
    sources::AgeFilter,
    store::{favorites::Favorites, important::ImportantSessions, settings::UiSettings},
    tui::App,
};
use std::{collections::HashSet, sync::mpsc::Sender};

pub(in crate::tui) struct AgeLink {
    sender: Sender<AgeFilter>,
    sent: AgeFilter,
}

pub(in crate::tui) fn starting_filter(
    settings: UiSettings,
    favorites: &Favorites,
    important: &ImportantSessions,
) -> AgeFilter {
    let keep = favorites.ids().chain(important.ids()).cloned().collect();
    AgeFilter::new(settings.max_age(), keep)
}

impl App {
    pub fn with_age_updates(mut self, sender: Sender<AgeFilter>, sent: AgeFilter) -> Self {
        self.age_link = Some(AgeLink { sender, sent });
        self
    }

    fn age_filter(&self) -> AgeFilter {
        let mut keep: HashSet<String> = self
            .favorites
            .ids()
            .chain(self.important.ids())
            .cloned()
            .collect();
        keep.extend(
            self.catalog
                .live()
                .iter()
                .filter_map(|live| live.session_id.clone()),
        );
        keep.extend(self.catalog.activity().keys().cloned());
        keep.extend(
            self.sessions
                .all()
                .iter()
                .filter_map(|session| session.session_id.clone()),
        );
        keep.extend(self.selected_entry().map(|entry| entry.session_id.clone()));
        AgeFilter::new(self.split.settings().max_age(), keep)
    }

    pub(in crate::tui) fn publish_age_filter(&mut self) {
        if self.age_link.is_none() {
            return;
        }
        let wanted = self.age_filter();
        if let Some(link) = self.age_link.as_mut()
            && link.sent != wanted
            && link.sender.send(wanted.clone()).is_ok()
        {
            link.sent = wanted;
        }
    }
}
