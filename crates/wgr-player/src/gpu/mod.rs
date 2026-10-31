//! Backend GPU: tradução do protocolo D3D11 emulado para wgpu, na mesma
//! divisão do `wgpu_worker.js`.

pub mod assembly;
pub mod bind;
pub mod formats;
pub mod layout;
pub mod objects;
pub mod pipeline;
pub mod pipeline_state;
pub mod recipe;
pub mod resources;
pub mod shaders;
pub mod state;
pub mod table;
pub mod upload;
