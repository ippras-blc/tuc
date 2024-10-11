use crate::Event;
use async_channel::Sender;
use esp_idf_svc::{hal::gpio::ADCPin, timer::EspAsyncTimer};
use led::{BLUE, RED, RGB8};
use log::{debug, error};
use std::time::Duration;
use time::OffsetDateTime;
use timed::Timed;
use turbidimeter::Turbidimeter;

// Every duration
const DURATION: Duration = Duration::from_secs(1);

pub(crate) async fn reader(
    turbidimeter: &mut Turbidimeter<'_>,
    mut timer: EspAsyncTimer,
    event_sender: &Sender<Event>,
    led_sender: &Sender<RGB8>,
) {
    turbidimeter.driver.start().unwrap();
    loop {
        timer.every(DURATION);
        while timer.tick().await.is_ok() {
            match turbidimeter.read::<1>().await {
                Ok([turbidity]) => {
                    led_sender.send(BLUE).await.ok();
                    event_sender
                        .send(Event::Turbidity(Timed {
                            time: OffsetDateTime::now_utc(),
                            value: turbidity,
                        }))
                        .await
                        .ok();
                    debug!("Turbidity={turbidity}");
                }
                Err(error) => {
                    led_sender.send(RED).await.ok();
                    error!("{error}");
                }
            }
        }
    }
}
