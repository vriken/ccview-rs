use crate::store::on_hold::OnHoldSessions;
use crate::tui::App;

impl App {
    pub(crate) fn with_on_hold(mut self, on_hold: OnHoldSessions) -> Self {
        self.on_hold = on_hold;
        self.refresh_rows();
        self
    }

    pub(in crate::tui) fn toggle_on_hold(&mut self) {
        if self.tab_bar_has_focus() {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        let was_on_hold = self.on_hold.contains_entry(&entry);
        match self.on_hold.toggle_entry(&entry) {
            Ok(()) => {
                self.notice = Some(
                    if was_on_hold {
                        "on hold cleared"
                    } else {
                        "on hold"
                    }
                    .to_owned(),
                );
            }
            Err(err) => self.notice = Some(format!("could not save on hold sessions: {err}")),
        }
        self.refresh_rows();
    }

    pub(in crate::tui) fn release_answered_holds(&mut self) {
        if let Err(err) = self.on_hold.reconcile(self.catalog.entries()) {
            self.notice = Some(format!("could not save on hold sessions: {err}"));
        }
    }
}
