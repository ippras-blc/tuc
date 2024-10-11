use crate::Event;
use async_channel::Sender;
use esp_idf_svc::{hal::gpio::IOPin, timer::EspAsyncTimer};
use led::{GREEN, RED, RGB8};
use log::{debug, error};
use std::time::Duration;
use thermometer::Thermometer;
use time::OffsetDateTime;
use timed::Timed;

// Every duration
const DURATION: Duration = Duration::from_secs(1);
const RETRY: usize = 2;

pub(crate) async fn reader(
    thermometer: &mut Thermometer<'_, impl IOPin>,
    mut timer: EspAsyncTimer,
    event_sender: &Sender<Event>,
    led_sender: &Sender<RGB8>,
) {
    loop {
        timer.every(DURATION);
        for _ in timer.tick().await {
            for _ in 0..RETRY {
                match thermometer.temperature().await {
                    Ok(temperature) => {
                        led_sender.send(GREEN).await.ok();
                        event_sender
                            .send(Event::Temperature(Timed {
                                time: OffsetDateTime::now_utc(),
                                value: temperature,
                            }))
                            .await
                            .ok();
                        debug!("Temperature={temperature}");
                        break;
                    }
                    Err(error) => {
                        led_sender.send(RED).await.ok();
                        error!("{error}");
                    }
                }
            }
        }
        // timer.after(SLEEP).await.ok();
    }
}
