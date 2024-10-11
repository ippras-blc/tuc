use crate::{WIFI_PASSWORD, WIFI_SSID};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{
        modem::{Modem, WifiModemPeripheral},
        peripheral::Peripheral,
    },
    nvs::EspDefaultNvsPartition,
    sys::EspError,
    timer::EspTaskTimerService,
    wifi::{AsyncWifi, ClientConfiguration, Configuration as WifiConfig, EspWifi},
};
use futures_lite::future::block_on;
use log::{error, info};
use std::{borrow::BorrowMut, thread, time::Duration};

/// WiFi reinitialize default timeout
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

// pub(crate) fn loop_initialize<'a>(
//     modem: &'a mut Modem,
//     event_loop: &EspSystemEventLoop,
//     timer_service: &EspTaskTimerService,
//     nvs: &EspDefaultNvsPartition,
// ) -> AsyncWifi<EspWifi<'a>> {
//     loop {
//         match initialize(&mut *modem, event_loop, timer_service, nvs) {
//             Ok(wifi) => break wifi,
//             Err(error) => {
//                 thread::sleep(DEFAULT_TIMEOUT);
//                 error!("{error}");
//             }
//         }
//     }
// }

/// WiFi initialize
pub(crate) fn initialize<'a>(
    modem: impl Peripheral<P = impl WifiModemPeripheral> + 'a,
    event_loop: &EspSystemEventLoop,
    timer_service: &EspTaskTimerService,
    nvs: &EspDefaultNvsPartition,
) -> Result<AsyncWifi<EspWifi<'a>>, EspError> {
    block_on(try_initialize(modem, event_loop, timer_service, nvs))
}

pub(crate) async fn try_initialize<'a>(
    modem: impl Peripheral<P = impl WifiModemPeripheral> + 'a,
    event_loop: &EspSystemEventLoop,
    timer_service: &EspTaskTimerService,
    nvs: &EspDefaultNvsPartition,
) -> Result<AsyncWifi<EspWifi<'a>>, EspError> {
    info!("initialize wifi");
    let wifi = EspWifi::new(modem, event_loop.clone(), Some(nvs.clone()))?;
    info!("sync wifi created");
    let mut wifi = AsyncWifi::wrap(wifi, event_loop.clone(), timer_service.clone())?;
    info!("async wifi created");
    wifi.set_configuration(&WifiConfig::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().unwrap(),
        password: WIFI_PASSWORD.try_into().unwrap(),
        ..Default::default()
    }))?;
    info!("wifi configured");
    wifi.start().await?;
    info!("wifi started");
    wifi.connect().await?;
    info!("wifi connected");
    wifi.wait_netif_up().await?;
    info!("wifi netif up");
    Ok(wifi)
}
