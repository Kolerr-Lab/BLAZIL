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

#[cfg(all(feature = "tigerbeetle-client", feature = "metrics-ws"))]
pub mod vsr_failover_scenario;

#[cfg(all(target_os = "linux", feature = "io-uring"))]
pub mod io_uring_udp_scenario;

#[cfg(all(target_os = "linux", feature = "af-xdp", feature = "metrics-ws"))]
pub mod afxdp_e2e_scenario;
