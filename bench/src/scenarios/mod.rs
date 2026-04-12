pub mod pipeline_scenario;
pub mod ring_buffer_scenario;
pub mod sharded_pipeline_scenario;
pub mod tcp_scenario;
pub mod tigerbeetle_scenario;
pub mod udp_scenario;

#[cfg(feature = "aeron")]
pub mod aeron_scenario;

#[cfg(feature = "tigerbeetle-client")]
pub mod sharded_tb_scenario;

#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub mod io_uring_udp_scenario;
