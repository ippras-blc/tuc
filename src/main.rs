use anyhow::Result;
use async_channel::bounded;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{
        adc::{attenuation::DB_11, Adc},
        modem::WifiModemPeripheral,
        peripheral::Peripheral,
        prelude::*,
        uart::{config::Config as UartConfig, AsyncUartDriver, Uart, UartDriver},
        units::Hertz,
    },
    log::EspLogger,
    nvs::EspDefaultNvsPartition,
    sntp::{EspSntp, SyncStatus},
    sys::link_patches,
    timer::EspTimerService,
    wifi::WifiEvent,
};
use executor::LocalExecutor;
use futures_lite::future::block_on;
use led::{Led, BLUE, GREEN, RED};
use log::{debug, error, info, warn};
use std::{process::exit, thread, time::Duration};
use thermometer::Thermometer;
use timed::Timed;
use turbidimeter::Turbidimeter;

const CHANNEL_CAPACITY: usize = 255;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);
const WIFI_REINITIALIZE_TIMEOUT: Duration = DEFAULT_TIMEOUT;
const SNTP_CHECK_STATUS_TIMEOUT: Duration = DEFAULT_TIMEOUT;

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

// turbidimeter
fn main() -> Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    link_patches();
    // Bind the log crate to the ESP Logging facilities
    EspLogger::initialize_default();
    info!("Initialize");

    let mut peripherals = Peripherals::take()?;
    let timer_service = EspTimerService::new()?;
    let event_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // Event loop
    let _subscription = event_loop.subscribe::<WifiEvent, _>(|event| {
        error!("Got event: {event:?}");
        if let WifiEvent::StaDisconnected = event {
            exit(-1);
        }
    })?;

    // LED
    let mut led = Led::new(peripherals.pins.gpio8, peripherals.rmt.channel0)?;
    info!("Led initialized");

    // Thermometer
    let mut thermometer = Thermometer::new(peripherals.pins.gpio2)?;
    info!("Thermometer initialized");

    // Turbidimeter
    // https://www.reddit.com/r/esp32/comments/1b6fles/adc2_is_no_longer_supported_please_use_adc1
    let mut turbidimeter = Turbidimeter::new(peripherals.adc1, peripherals.pins.gpio1)?;
    info!("Turbidimeter initialized");

    // Keep it around or else the WiFi service will stop
    let _wifi = wifi::sync::initialize!(&mut peripherals.modem, &event_loop, &timer_service, &nvs);
    info!("WiFi initialized");
    // Keep it around or else the SNTP service will stop
    let _sntp = sntp::initialize()?;
    info!("SNTP initialized");
    let (client, connection) = mqtt::initialize()?;
    info!("MQTT initialized");

    let executor = LocalExecutor::new();

    // LED blinker task
    let timer = timer_service.timer_async()?;
    let (led_sender, led_receiver) = bounded(CHANNEL_CAPACITY);
    executor.spawn(led.blinker(timer, led_receiver));

    // MQTT subscriber
    executor.spawn(mqtt::subscriber(connection));

    // MQTT publisher
    let timer = timer_service.timer_async()?;
    let (event_sender, event_receiver) = bounded(CHANNEL_CAPACITY);
    executor.spawn(mqtt::publisher(client, timer, event_receiver));

    // Temperature reader task
    let timer = timer_service.timer_async()?;
    executor.spawn(temperature::reader(
        &mut thermometer,
        timer,
        &event_sender,
        &led_sender,
    ));

    // Turbidity reader task
    let timer = timer_service.timer_async()?;
    executor.spawn(turbidity::reader(
        &mut turbidimeter,
        timer,
        &event_sender,
        &led_sender,
    ));

    block_on(executor.run());
    Ok(())
}

/// Event
#[derive(Clone, Copy, Debug)]
enum Event {
    Temperature(Timed<f32>),
    Turbidity(Timed<u16>),
}

mod mqtt;
mod temperature;
mod turbidity;
