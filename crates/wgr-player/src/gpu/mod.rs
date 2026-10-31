//! Backend GPU: tradução do protocolo D3D11 emulado para wgpu, na mesma
//! divisão do `wgpu_worker.js`.

pub mod bind;
pub mod formats;
pub mod recipe;
pub mod resources;
pub mod table;
