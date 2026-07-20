//! Slice 1: Ethernet (RMII) on the Olimex ESP32-POE-ISO.
//!
//! Fixed pin assignment of the board (see `docs/HARDWARE.md`):
//! RMII data 25/26/27/19/21/22 (fixed by ESP32-EMAC), MDC=GPIO23, MDIO=GPIO18,
//! REF-CLK as **inverted** output on GPIO17, PHY power/reset=GPIO12 (NEVER use as
//! general IO!), PHY=LAN8710A (LAN87XX driver), PHY address 0.
//!
//! DHCP sets the hostname `telenot-bridge` (option 12) so the headless device is
//! discoverable in the router/network.

use esp_idf_svc::eth::{BlockingEth, EspEth, EthDriver, RmiiClockConfig, RmiiEth, RmiiEthChipset};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio;
use esp_idf_svc::hal::mac::MAC;
use esp_idf_svc::ipv4;
use esp_idf_svc::netif::{EspNetif, NetifConfiguration};
use esp_idf_svc::sys::EspError;

pub const HOSTNAME: &str = "telenot-bridge";

/// The running Ethernet handle — must be kept alive, otherwise the link goes down.
pub type Eth<'d> = BlockingEth<EspEth<'d, RmiiEth>>;

/// Peripherals exclusively needed by the RMII driver (separated from other pins so
/// `main` can still freely assign UART/button/LED).
pub struct EthPeripherals<'d> {
    pub mac: MAC<'d>,
    pub rmii_rxd0: gpio::Gpio25<'d>,
    pub rmii_rxd1: gpio::Gpio26<'d>,
    pub rmii_crs_dv: gpio::Gpio27<'d>,
    pub rmii_txd1: gpio::Gpio22<'d>,
    pub rmii_tx_en: gpio::Gpio21<'d>,
    pub rmii_txd0: gpio::Gpio19<'d>,
    pub mdc: gpio::Gpio23<'d>,
    pub mdio: gpio::Gpio18<'d>,
    pub ref_clk: gpio::Gpio17<'d>,
    pub phy_power: gpio::Gpio12<'d>,
}

/// Brings up Ethernet, blocks until link + DHCP are up, and logs the IP.
pub fn bring_up<'d>(
    p: EthPeripherals<'d>,
    sysloop: EspSystemEventLoop,
) -> Result<Eth<'d>, EspError> {
    let driver = EthDriver::new_rmii(
        p.mac,
        p.rmii_rxd0,
        p.rmii_rxd1,
        p.rmii_crs_dv,
        p.mdc,
        p.rmii_txd1,
        p.rmii_tx_en,
        p.rmii_txd0,
        p.mdio,
        RmiiClockConfig::OutputInvertedGpio17(p.ref_clk),
        Some(p.phy_power),
        RmiiEthChipset::LAN87XX,
        Some(0),
        sysloop.clone(),
    )?;

    // Custom netif with DHCP hostname instead of the default.
    let netif = EspNetif::new_with_conf(&NetifConfiguration {
        ip_configuration: Some(ipv4::Configuration::Client(
            ipv4::ClientConfiguration::DHCP(ipv4::DHCPClientSettings {
                hostname: Some(HOSTNAME.try_into().expect("Hostname passt in 30 Zeichen")),
            }),
        )),
        ..NetifConfiguration::eth_default_client()
    })?;

    let mut eth = BlockingEth::wrap(EspEth::wrap_all(driver, netif)?, sysloop)?;

    log::info!("Ethernet: starte, warte auf Link + DHCP …");
    eth.start()?;
    eth.wait_netif_up()?;

    let ip = eth.eth().netif().get_ip_info()?;
    log::info!(
        "Ethernet UP — IP {} / Maske {} / GW {} / Hostname {HOSTNAME}",
        ip.ip,
        ip.subnet.mask,
        ip.subnet.gateway,
    );

    Ok(eth)
}
