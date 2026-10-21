use anyhow::{Context, bail, ensure};
use esp_idf_svc::hal::delay::{FreeRtos, TickType};
use esp_idf_svc::hal::i2c::I2cDriver;

const AHT20: u8 = 0x38;
const BMP280_ADDRESSES: [u8; 2] = [0x77, 0x76];
const BMP280_CHIP_ID: u8 = 0x58;
const TIMEOUT_MS: u64 = 100;
/// The first transfer after a reset can time out.
const STATUS_TRIES: u8 = 3;
/// Both sensors measure at once; the AHT20 is the slower one.
const MEASUREMENT_MS: u32 = 80;
/// Temperature oversampling ×2, pressure ×16, forced mode (one measurement, then sleep).
const BMP280_FORCED_MEASUREMENT: u8 = 0b010_101_01;

pub struct Measurement {
    pub temperature: f32,
    pub humidity: f32,
    pub pressure: f32,
}

/// The AHT20 + BMP280 module: temperature and humidity from the AHT20, pressure from the BMP280
/// (its own temperature only feeds the pressure compensation: the chip warms itself).
pub struct Aht20Bmp280 {
    i2c: I2cDriver<'static>,
    bmp280: u8,
    calibration: Calibration,
}

impl Aht20Bmp280 {
    pub fn new(mut i2c: I2cDriver<'static>) -> anyhow::Result<Self> {
        let mut status = [0];
        let mut answer = Ok(());
        for _ in 0..STATUS_TRIES {
            FreeRtos::delay_ms(40);
            answer = i2c.write_read(AHT20, &[0x71], &mut status, timeout());
            if answer.is_ok() {
                break;
            }
        }
        if let Err(error) = answer {
            bail!("AHT20 not answering ({error}); {}", scan(&mut i2c));
        }
        if status[0] & 0x08 == 0 {
            i2c.write(AHT20, &[0xBE, 0x08, 0x00], timeout())?;
            FreeRtos::delay_ms(10);
        }
        let bmp280 = BMP280_ADDRESSES
            .into_iter()
            .find(|&address| {
                let mut id = [0];
                i2c.write_read(address, &[0xD0], &mut id, timeout()).is_ok()
                    && id[0] == BMP280_CHIP_ID
            })
            .with_context(|| format!("BMP280 not answering; {}", scan(&mut i2c)))?;
        let mut raw = [0; 24];
        i2c.write_read(bmp280, &[0x88], &mut raw, timeout())?;
        Ok(Self {
            i2c,
            bmp280,
            calibration: Calibration::from_registers(&raw),
        })
    }

    pub fn measure(&mut self) -> anyhow::Result<Measurement> {
        self.i2c.write(AHT20, &[0xAC, 0x33, 0x00], timeout())?;
        self.i2c
            .write(self.bmp280, &[0xF4, BMP280_FORCED_MEASUREMENT], timeout())?;
        FreeRtos::delay_ms(MEASUREMENT_MS);

        let mut aht20 = [0; 7];
        self.i2c.read(AHT20, &mut aht20, timeout())?;
        ensure!(aht20[0] & 0x80 == 0, "AHT20 still busy");
        ensure!(crc8(&aht20[..6]) == aht20[6], "AHT20 checksum mismatch");
        let humidity_raw =
            u32::from(aht20[1]) << 12 | u32::from(aht20[2]) << 4 | u32::from(aht20[3]) >> 4;
        let temperature_raw =
            u32::from(aht20[3] & 0x0F) << 16 | u32::from(aht20[4]) << 8 | u32::from(aht20[5]);

        let mut bmp280 = [0; 6];
        self.i2c
            .write_read(self.bmp280, &[0xF7], &mut bmp280, timeout())?;
        let pressure_raw = twenty_bits(&bmp280[0..3]);
        let bmp280_temperature_raw = twenty_bits(&bmp280[3..6]);
        let Some(pressure) = self
            .calibration
            .pressure_pascal(bmp280_temperature_raw, pressure_raw)
        else {
            bail!("BMP280 gave an impossible reading");
        };

        Ok(Measurement {
            temperature: (f64::from(temperature_raw) * 200.0 / 1_048_576.0 - 50.0) as f32,
            humidity: (f64::from(humidity_raw) * 100.0 / 1_048_576.0) as f32,
            pressure: (pressure / 100.0) as f32,
        })
    }
}

/// The addresses that answer, to tell a wiring problem from another chip.
fn scan(i2c: &mut I2cDriver<'static>) -> String {
    let found: Vec<String> = (0x08..0x78)
        .filter(|&address| i2c.write(address, &[], timeout()).is_ok())
        .map(|address| format!("0x{address:02x}"))
        .collect();
    if found.is_empty() {
        "nothing answers on the bus".into()
    } else {
        format!("answering: {}", found.join(", "))
    }
}

fn timeout() -> u32 {
    TickType::new_millis(TIMEOUT_MS).ticks()
}

fn twenty_bits(bytes: &[u8]) -> i32 {
    i32::from(bytes[0]) << 12 | i32::from(bytes[1]) << 4 | i32::from(bytes[2]) >> 4
}

/// CRC-8, polynomial 0x31, initial value 0xFF.
fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0xFF_u8;
    for byte in bytes {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                crc << 1 ^ 0x31
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// The BMP280 trimming parameters; the compensation is the datasheet's floating-point one.
struct Calibration {
    temperature: [f64; 3],
    pressure: [f64; 9],
}

impl Calibration {
    fn from_registers(raw: &[u8; 24]) -> Self {
        let unsigned = |index: usize| f64::from(u16::from_le_bytes([raw[index], raw[index + 1]]));
        let signed = |index: usize| f64::from(i16::from_le_bytes([raw[index], raw[index + 1]]));
        Self {
            temperature: [unsigned(0), signed(2), signed(4)],
            pressure: [
                unsigned(6),
                signed(8),
                signed(10),
                signed(12),
                signed(14),
                signed(16),
                signed(18),
                signed(20),
                signed(22),
            ],
        }
    }

    fn pressure_pascal(&self, temperature_raw: i32, pressure_raw: i32) -> Option<f64> {
        let [t1, t2, t3] = self.temperature;
        let [p1, p2, p3, p4, p5, p6, p7, p8, p9] = self.pressure;
        let temperature_raw = f64::from(temperature_raw);
        let fine_temperature = (temperature_raw / 16384.0 - t1 / 1024.0) * t2
            + (temperature_raw / 131072.0 - t1 / 8192.0).powi(2) * t3;

        let mut var1 = fine_temperature / 2.0 - 64000.0;
        let mut var2 = var1 * var1 * p6 / 32768.0 + var1 * p5 * 2.0;
        var2 = var2 / 4.0 + p4 * 65536.0;
        var1 = (p3 * var1 * var1 / 524288.0 + p2 * var1) / 524288.0;
        var1 = (1.0 + var1 / 32768.0) * p1;
        if var1 == 0.0 {
            return None;
        }
        let mut pressure = 1_048_576.0 - f64::from(pressure_raw);
        pressure = (pressure - var2 / 4096.0) * 6250.0 / var1;
        let var1 = p9 * pressure * pressure / 2_147_483_648.0;
        let var2 = pressure * p8 / 32768.0;
        Some(pressure + (var1 + var2 + p7) / 16.0)
    }
}
