use self::{
    priority_executor::{Priority, PriorityExecutor},
    simple_executor::SimpleExecutor,
};
use anyhow::{Error, Result};
use async_channel::bounded;
use async_mutex::Mutex;
use async_rwlock::RwLock;
use blc::Timed;
use edge_executor::{LocalExecutor, Task};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{
        adc::{attenuation::DB_11, Adc},
        delay::Delay,
        gpio::{
            ADCPin, Gpio0, Gpio1, GpioError, IOPin, InputOutput, InputPin, OutputPin, Pin,
            PinDriver,
        },
        modem::WifiModemPeripheral,
        peripheral::Peripheral,
        prelude::*,
        uart::{config::Config as UartConfig, AsyncUartDriver, Uart, UartDriver},
        units::Hertz,
    },
    log::EspLogger,
    mqtt::client::{EspAsyncMqttClient, EspAsyncMqttConnection, MqttClientConfiguration, QoS},
    nvs::EspDefaultNvsPartition,
    sntp::{EspSntp, SyncStatus},
    sys::{link_patches, EspError},
    timer::{EspAsyncTimer, EspTaskTimerService, EspTimerService},
    wifi::{AsyncWifi, ClientConfiguration, Configuration as WifiConfig, EspWifi, WifiEvent},
};
use futures_lite::{
    future::{block_on, yield_now},
    FutureExt,
};
use led::{Led, BLACK, BLUE, GREEN, RED};
use log::{debug, error, info, warn};
use ron::{
    extensions::Extensions,
    from_str,
    ser::{to_string, PrettyConfig},
};
use std::{pin::pin, sync::Arc, thread, time::Duration};
use thermometer::Thermometer;
use time::OffsetDateTime;
use turbidity_sensor::TurbiditySensor;

// export WIFI_SSID=""
const WIFI_SSID: &str = env!("WIFI_SSID");
// export WIFI_PASSWORD=""
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

const CHANNEL_BOUND: usize = 255;

const MQTT_URL: &str = "mqtt://broker.emqx.io:1883";
const MQTT_CLIENT_ID: &str = "ippras.ru/blc/7c:df:a1:61:f1:48";
const MQTT_TOPIC_BLC: &str = "ippras.ru/blc/#";
const MQTT_TOPIC_TEMPERATURE: &str = "ippras.ru/blc/temperature";
const MQTT_TOPIC_TURBIDITY: &str = "ippras.ru/blc/turbidity";
// export MQTT_USERNAME="7c:df:a1:61:f1:48"
const MQTT_USERNAME: Option<&str> = option_env!("MQTT_USERNAME");
// export MQTT_PASSWORD="12345"
const MQTT_PASSWORD: Option<&str> = option_env!("MQTT_PASSWORD");

const SLEEP_SECS: u64 = 2;
const RETRY_SLEEP_MSECS: u64 = 500;

#[derive(Clone, Copy, Debug)]
enum Event {
    Temperature(Timed<f32>),
    Turbidity(Timed<u16>),
}

fn main() -> Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    link_patches();
    // Bind the log crate to the ESP Logging facilities
    EspLogger::initialize_default();
    info!("Initialize");

    let peripherals = Peripherals::take()?;
    let timer_service = EspTimerService::new()?;
    let event_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    // let led = &Arc::new(Mutex::new(Led::new(
    //     peripherals.pins.gpio8,
    //     peripherals.rmt.channel0,
    // )?));
    let led = Led::new(peripherals.pins.gpio8, peripherals.rmt.channel0)?;
    info!("led initialized");
    let mut thermometer = Thermometer::new(peripherals.pins.gpio2)?;
    info!("thermometer initialized");
    // https://www.reddit.com/r/esp32/comments/1b6fles/adc2_is_no_longer_supported_please_use_adc1
    let mut turbidity_sensor = TurbiditySensor::new(peripherals.adc1, peripherals.pins.gpio1)?;
    info!("turbidity sensor initialized");
    let _wifi = block_on(initialize_wifi(
        peripherals.modem,
        &event_loop,
        &timer_service,
        &nvs,
    ))?;
    info!("WiFi initialized");
    // Keep it around or else the SNTP service will stop
    let sntp = EspSntp::new_default()?;
    while SyncStatus::Completed != sntp.get_sync_status() {
        debug!("SNTP is not completed, wait");
        thread::sleep(Duration::from_secs(1));
    }
    info!("SNTP initialized");
    let (mut client, mut connection) = initialize_mqtt(MQTT_URL, MQTT_CLIENT_ID)?;
    info!("MQTT initialized");

    let (led_sender, led_receiver) = bounded(CHANNEL_BOUND);
    let (event_sender, event_receiver) = bounded(CHANNEL_BOUND);
    let executor = SimpleExecutor::new();
    // Subscriber
    executor.spawn(async move {
        while let Ok(event) = connection.next().await {
            info!("Subscribed: {}", event.payload());
        }
        warn!("MQTT connection closed");
    });
    // Publisher
    let mut timer = timer_service.timer_async()?;
    executor.spawn(async move {
        loop {
            if let Err(error) = client.subscribe(MQTT_TOPIC_BLC, QoS::ExactlyOnce).await {
                warn!(r#"Failed to subscribe to topic "{MQTT_TOPIC_BLC}": {error}, retrying..."#);
                // Retry in 0.5s
                timer.after(Duration::from_millis(500)).await?;
                continue;
            }
            info!(r#"Subscribed to topic "{MQTT_TOPIC_BLC}""#);
            // Just to give a chance of our connection to get even the first published message
            timer.after(Duration::from_millis(500)).await?;
            while let Ok(event) = &event_receiver.recv().await {
                match event {
                    Event::Temperature(temperature) => {
                        let serialized = ron::to_string(temperature)?;
                        client
                            .publish(
                                MQTT_TOPIC_TEMPERATURE,
                                QoS::ExactlyOnce,
                                false,
                                serialized.as_bytes(),
                            )
                            .await?;
                        info!(r#"Published "{serialized}" to topic "{MQTT_TOPIC_TEMPERATURE}""#);
                    }
                    Event::Turbidity(turbidity) => {
                        let serialized = ron::to_string(turbidity)?;
                        client
                            .publish(
                                MQTT_TOPIC_TURBIDITY,
                                QoS::ExactlyOnce,
                                false,
                                serialized.as_bytes(),
                            )
                            .await?;
                        info!(r#"Published "{serialized:?}" to topic "{MQTT_TOPIC_TURBIDITY}""#);
                    }
                }
            }
            warn!("Channel closed");
        }
        Ok::<_, Error>(())
    });
    // LED blinker task
    let timer = timer_service.timer_async()?;
    executor.spawn(blinker::task(led, timer, led_receiver));
    // Temperature task
    let timer = timer_service.timer_async()?;
    executor.spawn(temperature::task(
        thermometer,
        timer,
        event_sender.clone(),
        led_sender.clone(),
    ));
    // executor.spawn(async move {
    //     loop {
    //         for _ in 0..2 {
    //             match thermometer.temperature().await {
    //                 Ok(temperature) => {
    //                     led.lock_arc().await.set_color(GREEN)?;
    //                     sender
    //                         .send(Event::Temperature(Timed {
    //                             time: OffsetDateTime::now_utc(),
    //                             value: temperature,
    //                         }))
    //                         .await?;
    //                     debug!("Temperature={temperature}");
    //                 }
    //                 Err(error) => {
    //                     led.lock_arc().await.set_color(RED)?;
    //                     error!("{error}");
    //                     if error.is_crc() {
    //                         continue;
    //                     }
    //                 }
    //             }
    //             break;
    //         }
    //         led.lock_arc().await.set_color(BLACK)?;
    //         timer.after(Duration::from_secs(SLEEP_SECS)).await?;
    //     }
    //     Ok::<_, Error>(())
    // });

    // Turbidity task
    let mut timer = timer_service.timer_async()?;
    let sender = event_sender.clone();
    executor.spawn(async move {
        loop {
            match turbidity_sensor.read() {
                Ok(turbidity) => {
                    // led.lock_arc().await.set_color(BLUE)?;
                    sender
                        .send(Event::Turbidity(Timed {
                            time: OffsetDateTime::now_utc(),
                            value: turbidity,
                        }))
                        .await?;
                    debug!("Turbidity={turbidity}");
                }
                Err(error) => {
                    // led.lock_arc().await.set_color(RED)?;
                    error!("{error}");
                }
            }
            // led.lock_arc().await.set_color(BLACK)?;
            timer.after(Duration::from_secs_f32(1.5)).await?;
        }
        Ok::<_, Error>(())
    });
    block_on(executor.run());
    Ok(())
}

// fn serialize<T: Serialize>(path: &Path, deserialized: Timed<T>) -> Result<()> {
//     let serialized = to_string_pretty(
//         deserialized,
//         PrettyConfig::default().extensions(Extensions::IMPLICIT_SOME),
//     )?;
//     write(&path, serialized)?;
//     Ok(())
// }

async fn initialize_wifi<'a>(
    modem: impl Peripheral<P = impl WifiModemPeripheral> + 'a,
    event_loop: &EspSystemEventLoop,
    timer_service: &EspTaskTimerService,
    nvs: &EspDefaultNvsPartition,
) -> Result<EspWifi<'a>, EspError> {
    info!("initialize wifi");
    let mut esp_wifi = EspWifi::new(modem, event_loop.clone(), Some(nvs.clone()))?;
    let mut wifi = AsyncWifi::wrap(&mut esp_wifi, event_loop.clone(), timer_service.clone())?;
    wifi.set_configuration(&WifiConfig::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().unwrap(),
        password: WIFI_PASSWORD.try_into().unwrap(),
        ..Default::default()
    }))?;
    wifi.start().await?;
    info!("wifi started");
    wifi.connect().await?;
    info!("wifi connected");
    wifi.wait_netif_up().await?;
    info!("wifi netif up");
    Ok(esp_wifi)
}

fn initialize_mqtt(
    url: &str,
    id: &str,
) -> Result<(EspAsyncMqttClient, EspAsyncMqttConnection), EspError> {
    info!("initialize mqtt");
    Ok(EspAsyncMqttClient::new(
        url,
        &MqttClientConfiguration {
            client_id: Some(id),
            username: MQTT_USERNAME,
            password: MQTT_PASSWORD,
            ..Default::default()
        },
    )?)
}

mod blinker;
mod priority_executor;
mod simple_executor;
mod temperature;
