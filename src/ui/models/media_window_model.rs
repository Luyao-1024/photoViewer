use crate::core::error::Result;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::{MediaQuery, MediaRepository};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::collections::HashSet;

pub struct MediaWindowModel {
    query: MediaQuery,
    page_size: u32,
    window_start: u32,
    total: u32,
    generation: u64,
    ids_in_window: Vec<MediaId>,
    selected: HashSet<MediaId>,
    store: gtk::gio::ListStore,
}

impl MediaWindowModel {
    pub fn new(query: MediaQuery, page_size: u32) -> Self {
        Self {
            query,
            page_size,
            window_start: 0,
            total: 0,
            generation: 0,
            ids_in_window: Vec::new(),
            selected: HashSet::new(),
            store: gtk::gio::ListStore::new::<glib::BoxedAnyObject>(),
        }
    }

    pub fn load_sync(&mut self, repo: &MediaRepository, start: u32) -> Result<()> {
        let page = repo.page(self.query.clone(), start, self.page_size)?;
        self.window_start = page.start;
        self.total = page.total;
        self.generation = self.generation.saturating_add(1);
        self.ids_in_window = page
            .items
            .iter()
            .map(|item| MediaId::from(item.id))
            .collect();
        replace_store_items(&self.store, page.items);
        Ok(())
    }

    pub fn store(&self) -> gtk::gio::ListStore {
        self.store.clone()
    }

    pub fn window_start(&self) -> u32 {
        self.window_start
    }

    pub fn total(&self) -> u32 {
        self.total
    }

    pub fn select(&mut self, id: MediaId) {
        self.selected.insert(id);
    }

    pub fn is_selected(&self, id: MediaId) -> bool {
        self.selected.contains(&id)
    }

    pub fn id_at_window_index(&self, index: u32) -> Option<MediaId> {
        self.ids_in_window.get(index as usize).copied()
    }

    pub fn next_generation_for_tests(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    pub fn generation_is_current_for_tests(&self, generation: u64) -> bool {
        self.generation == generation
    }
}

fn replace_store_items(store: &gtk::gio::ListStore, items: Vec<MediaItem>) {
    let additions: Vec<glib::BoxedAnyObject> =
        items.into_iter().map(glib::BoxedAnyObject::new).collect();
    store.splice(0, store.n_items(), &additions);
}
