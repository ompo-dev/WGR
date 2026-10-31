//! Tabela de objetos do protocolo: ids da engine mapeiam para texturas,
//! buffers, samplers e views. Os ids não são reciclados pela engine; recriar um
//! id substitui o objeto e invalida quem dependia dele.

#[derive(Debug, Default)]
pub struct ObjectTable<T> {
    slots: Vec<Option<T>>,
    alive: usize,
}

impl<T> ObjectTable<T> {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            alive: 0,
        }
    }

    /// Insere ou substitui o objeto do id, devolvendo o anterior.
    pub fn insert(&mut self, id: u32, value: T) -> Option<T> {
        let index = id as usize;
        if index >= self.slots.len() {
            self.slots.resize_with(index + 1, || None);
        }
        let previous = self.slots[index].replace(value);
        if previous.is_none() {
            self.alive += 1;
        }
        previous
    }

    pub fn get(&self, id: u32) -> Option<&T> {
        self.slots.get(id as usize).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        self.slots.get_mut(id as usize).and_then(Option::as_mut)
    }

    pub fn contains(&self, id: u32) -> bool {
        self.get(id).is_some()
    }

    pub fn remove(&mut self, id: u32) -> Option<T> {
        let value = self.slots.get_mut(id as usize)?.take();
        if value.is_some() {
            self.alive -= 1;
        }
        value
    }

    pub fn len(&self) -> usize {
        self.alive
    }

    pub fn is_empty(&self) -> bool {
        self.alive == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(id, slot)| slot.as_ref().map(|value| (id as u32, value)))
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
    }
}
