//! RAGE ring v1 decoder, not a GPU backend. Offsets include the 64-byte header.
use crate::shared::GuestMemory;
use anyhow::{Result, ensure};
const DATA: u32 = 64;
pub struct Command {
    pub opcode: u32,
    pub words: Vec<u32>,
    next: u32,
}
pub struct Ring {
    memory: GuestMemory,
    base: u64,
    size: u32,
    tail: u32,
}
impl Ring {
    pub fn new(memory: GuestMemory, base: u64, size: u32) -> Result<Self> {
        ensure!(
            base.is_multiple_of(4) && size >= 80 && size.is_multiple_of(4),
            "Ring GPU inválida"
        );
        ensure!(
            base.checked_add(u64::from(size))
                .is_some_and(|end| end <= memory.len() as u64),
            "Ring GPU fora da memória"
        );
        Ok(Self {
            memory,
            base,
            size,
            tail: DATA,
        })
    }
    /// Only the actual GPU backend may announce readiness.
    pub fn ready(&self) -> Result<()> {
        self.memory.store_u32(self.base + 4, DATA)?;
        self.memory.store_u32(self.base + 8, 1)
    }
    pub fn next_command(&self) -> Result<Option<Command>> {
        let head = self.memory.load_u32(self.base)?;
        ensure!(
            head >= DATA && head < self.size && head % 4 == 0,
            "Head GPU inválido"
        );
        if head == self.tail {
            return Ok(None);
        }
        ensure!(
            self.tail.checked_add(8).is_some_and(|end| end <= self.size),
            "Cabeçalho GPU truncado"
        );
        let at = self.base + u64::from(self.tail);
        let opcode = self.memory.load_u32(at)?;
        let count = self.memory.load_u32(at + 4)?;
        ensure!(opcode <= 47, "Opcode GPU desconhecido: {opcode}");
        if opcode == 1 {
            ensure!(self.tail > DATA, "WRAP GPU no início");
            return Ok(Some(Command {
                opcode,
                words: vec![],
                next: DATA,
            }));
        }
        ensure!(count <= 32768, "Comando GPU excede 128 KiB");
        let next = self
            .tail
            .checked_add(8 + count * 4)
            .filter(|next| *next < self.size)
            .ok_or_else(|| anyhow::anyhow!("Payload GPU truncado"))?;
        if head > self.tail {
            ensure!(next <= head, "Payload GPU ainda não publicado");
        }
        let words = (0..count)
            .map(|i| self.memory.load_u32(at + 8 + u64::from(i) * 4))
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(Command {
            opcode,
            words,
            next,
        }))
    }
    /// Advance only after the backend has consumed the referenced data.
    pub fn acknowledge(&mut self, command: Command) -> Result<()> {
        self.tail = command.next;
        self.memory.add_u32(self.base + 12, 1)?;
        self.memory.store_u32(self.base + 4, self.tail)?;
        self.memory.notify(self.base + 4)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_unconsumed_data_and_rejects_truncation() {
        let runtime = crate::NativeRuntime::new().unwrap();
        let memory = GuestMemory::new64(&runtime.engine, 1, 1, 65536).unwrap();
        let mut ring = Ring::new(memory.clone(), 0, 4096).unwrap();
        memory.store_u32(0, 76).unwrap();
        memory.store_u32(64, 7).unwrap();
        memory.store_u32(68, 1).unwrap();
        memory.store_u32(72, 900).unwrap();
        ring.ready().unwrap();
        let command = ring.next_command().unwrap().unwrap();
        assert_eq!(command.opcode, 7);
        assert_eq!(command.words, [900]);
        assert_eq!(memory.load_u32(4).unwrap(), 64);
        ring.acknowledge(command).unwrap();
        assert_eq!(memory.load_u32(4).unwrap(), 76);
        assert!(ring.next_command().unwrap().is_none());
        memory.store_u32(0, 88).unwrap();
        memory.store_u32(76, 7).unwrap();
        memory.store_u32(80, 20).unwrap();
        assert!(ring.next_command().is_err());
    }
}
