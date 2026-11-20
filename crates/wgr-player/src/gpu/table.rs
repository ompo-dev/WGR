//! Tabela de objetos do protocolo: ids da engine mapeiam para texturas,
//! buffers, samplers e views. Os ids não são reciclados pela engine; recriar um
//! id substitui o objeto e invalida quem dependia dele.

use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct ObjectTable<T> {
    slots: BTreeMap<u32, T>,
}

impl<T> ObjectTable<T> {
    pub fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
        }
    }

    /// Insere ou substitui o objeto do id, devolvendo o anterior.
    pub fn insert(&mut self, id: u32, value: T) -> Option<T> {
        self.slots.insert(id, value)
    }

    pub fn get(&self, id: u32) -> Option<&T> {
        self.slots.get(&id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        self.slots.get_mut(&id)
    }

    pub fn contains(&self, id: u32) -> bool {
        self.get(id).is_some()
    }

    pub fn remove(&mut self, id: u32) -> Option<T> {
        self.slots.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.slots.iter().map(|(&id, value)| (id, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_and_remove_keep_the_live_count() {
        let mut table: ObjectTable<&'static str> = ObjectTable::new();
        assert_eq!(table.insert(3, "tex"), None);
        assert_eq!(table.insert(1, "buf"), None);
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(3), Some(&"tex"));
        assert_eq!(table.remove(3), Some("tex"));
        assert_eq!(table.len(), 1);
        assert!(!table.contains(3));
        assert!(table.get(1).is_some());
    }

    #[test]
    fn replacing_an_id_returns_the_previous_object_without_growing() {
        let mut table: ObjectTable<u32> = ObjectTable::new();
        table.insert(2, 10);
        assert_eq!(table.insert(2, 20), Some(10));
        assert_eq!(table.len(), 1);
        assert_eq!(table.get(2), Some(&20));
    }

    #[test]
    fn sparse_ids_stay_addressable() {
        let mut table: ObjectTable<u8> = ObjectTable::new();
        table.insert(10_000, 7);
        assert_eq!(table.get(10_000), Some(&7));
        assert!(table.get(9_999).is_none());
        assert_eq!(table.iter().map(|(id, _)| id).collect::<Vec<_>>(), [10_000]);
        table.insert(u32::MAX, 9);
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(u32::MAX), Some(&9));
        *table.get_mut(u32::MAX).unwrap() = 10;
        assert_eq!(table.remove(u32::MAX), Some(10));
        assert_eq!(table.len(), 1);
    }
}
