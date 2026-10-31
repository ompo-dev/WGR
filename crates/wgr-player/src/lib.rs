//! Player nativo do WGR: tradução Rust dos componentes do 5-web-port.
//!
//! O `play.html` descreve o bloco de input e o dimensionamento da tela, o
//! `loader.js` monta os argumentos da engine, e o `wgpu_worker.js` consome o
//! ring de comandos. Este crate começa por esses contratos, sem depender de
//! WebView, HTML ou JavaScript.

pub mod drain;
pub mod engine;
pub mod gpu;
pub mod input;
pub mod loader;
pub mod shader;
pub mod surface;
