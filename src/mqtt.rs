use crate::Event;
use anyhow::Result;
use async_channel::Receiver;
use esp_idf_svc::{
    mqtt::client::{EspAsyncMqttClient, EspAsyncMqttConnection, MqttClientConfiguration, QoS},
    sys::EspError,
    timer::EspAsyncTimer,
};
use log::{info, warn};
use std::time::Duration;

const MQTT_URL: &str = "mqtt://192.168.0.87:1883";
const MQTT_CLIENT_ID: &str = "f0:f5:bd:0e:fe:f8";
const MQTT_USERNAME: Option<&str> = option_env!("MQTT_USERNAME");
const MQTT_PASSWORD: Option<&str> = option_env!("MQTT_PASSWORD");

const MQTT_TOPIC_BLC: &str = "ippras.ru/blc/#";
const MQTT_TOPIC_TEMPERATURE: &str = "ippras.ru/blc/temperature";
const MQTT_TOPIC_TURBIDITY: &str = "ippras.ru/blc/turbidity";

const RETRY: Duration = Duration::from_millis(500);
const SLEEP: Duration = Duration::from_secs(1);

pub(crate) fn initialize() -> Result<(EspAsyncMqttClient, EspAsyncMqttConnection), EspError> {
    info!("initialize mqtt");
    Ok(EspAsyncMqttClient::new(
        MQTT_URL,
        &MqttClientConfiguration {
            client_id: Some(MQTT_CLIENT_ID),
            username: MQTT_USERNAME,
            password: MQTT_PASSWORD,
            ..Default::default()
        },
    )?)
}

// Subscriber
pub(crate) async fn subscriber(mut connection: EspAsyncMqttConnection) {
    while let Ok(event) = connection.next().await {
        info!("Subscribed: {}", event.payload());
    }
    warn!("MQTT connection closed");
}

// Publisher
pub(crate) async fn publisher(
    mut client: EspAsyncMqttClient,
    mut timer: EspAsyncTimer,
    event_receiver: Receiver<Event>,
) -> Result<()> {
    loop {
        if let Err(error) = client.subscribe(MQTT_TOPIC_BLC, QoS::ExactlyOnce).await {
            warn!(r#"Retry to subscribe to topic "{MQTT_TOPIC_BLC}": {error}"#);
            timer.after(RETRY).await?;
            continue;
        }
        info!(r#"Subscribed to topic "{MQTT_TOPIC_BLC}""#);
        // Just to give a chance of our connection to get even the first published message
        timer.after(SLEEP).await?;
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
                    info!(r#"Published "{serialized}" to topic "{MQTT_TOPIC_TURBIDITY}""#);
                }
            }
        }
        warn!("Channel closed");
    }
}
