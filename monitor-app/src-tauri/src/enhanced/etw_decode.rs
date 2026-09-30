//! Schema-based decoding, not hard-coded offsets. No session is started here.
use super::disk_io::{Accumulator, Direction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Disk,
    Process,
    Thread,
    Other,
}
#[derive(Clone, Copy)]
pub struct Header {
    pub provider: Provider,
    pub opcode: u8,
    pub version: u8,
    pub pointer_bytes: usize,
    pub timestamp: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    UnsupportedVersion,
    InvalidWidth,
    MissingProperty,
    InvalidValue,
    InvalidClock,
    Native(u32),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Irp,
    IssuingThreadId,
    TransferSize,
    ProcessId,
    ThreadId,
}
impl Field {
    #[cfg(target_os = "windows")]
    fn name(self) -> &'static str {
        match self {
            Self::Irp => "Irp",
            Self::IssuingThreadId => "IssuingThreadId",
            Self::TransferSize => "TransferSize",
            Self::ProcessId => "ProcessId",
            Self::ThreadId => "TThreadId",
        }
    }
}
#[derive(Clone, Copy)]
pub struct Scalar {
    pub value: u64,
    pub bytes: usize,
}
pub trait Fields {
    fn read(&mut self, field: Field) -> Result<Scalar, Error>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Start,
    End,
    PresentAtStart,
    PresentAtEnd,
}
impl Lifecycle {
    fn from_opcode(opcode: u8) -> Option<Self> {
        match opcode {
            1 => Some(Self::Start),
            2 => Some(Self::End),
            3 => Some(Self::PresentAtStart),
            4 => Some(Self::PresentAtEnd),
            _ => None,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Body {
    Begin {
        request: u64,
        tid: u32,
        direction: Direction,
    },
    Complete {
        request: u64,
        bytes: u32,
        direction: Direction,
    },
    Process {
        pid: u32,
        phase: Lifecycle,
    },
    Thread {
        pid: u32,
        tid: u32,
        phase: Lifecycle,
    },
}
#[derive(Debug)]
pub struct Event {
    pub at_ns: u64,
    pub body: Body,
}
impl Event {
    /// Only storage events can be forwarded without a separate verified lifecycle resolver.
    pub fn apply_disk(&self, accumulator: &mut Accumulator) -> bool {
        match self.body {
            Body::Begin {
                request,
                tid,
                direction,
            } => {
                accumulator.begin(self.at_ns, request, tid, direction);
                true
            }
            Body::Complete {
                request,
                bytes,
                direction,
            } => {
                accumulator.complete(self.at_ns, request, direction, bytes);
                true
            }
            _ => false,
        }
    }
}
#[derive(Clone, Copy)]
pub struct QpcClock {
    origin: i64,
    frequency: u64,
}
impl QpcClock {
    /// Caller must configure PROCESS_TRACE_MODE_RAW_TIMESTAMP and ClientContext=1.
    pub fn new(origin: i64, frequency: u64) -> Result<Self, Error> {
        if origin < 0 || frequency == 0 {
            return Err(Error::InvalidClock);
        }
        Ok(Self { origin, frequency })
    }
    pub fn nanoseconds(&self, ticks: i64) -> Result<u64, Error> {
        let delta = ticks
            .checked_sub(self.origin)
            .filter(|v| *v >= 0)
            .ok_or(Error::InvalidClock)? as u128;
        u64::try_from(delta * 1_000_000_000 / u128::from(self.frequency))
            .map_err(|_| Error::InvalidClock)
    }
}
fn scalar(
    fields: &mut impl Fields,
    field: Field,
    bytes: usize,
    nonzero: bool,
) -> Result<u64, Error> {
    let value = fields.read(field)?;
    if value.bytes != bytes || (bytes == 4 && value.value > u32::MAX as u64) {
        return Err(Error::InvalidWidth);
    }
    if nonzero && value.value == 0 {
        return Err(Error::InvalidValue);
    }
    Ok(value.value)
}
pub fn decode(
    header: Header,
    fields: &mut impl Fields,
    clock: &QpcClock,
) -> Result<Option<Event>, Error> {
    let storage = header.provider == Provider::Disk && matches!(header.opcode, 10..=13);
    let phase = Lifecycle::from_opcode(header.opcode);
    let lifecycle =
        matches!(header.provider, Provider::Process | Provider::Thread) && phase.is_some();
    if !storage && !lifecycle {
        return Ok(None);
    }
    if !matches!(header.pointer_bytes, 4 | 8) {
        return Err(Error::InvalidWidth);
    }
    let supported = match header.provider {
        Provider::Disk => header.version == 3,
        Provider::Process => matches!(header.version, 2..=4),
        Provider::Thread => matches!(header.version, 2 | 3),
        _ => false,
    };
    if !supported {
        return Err(Error::UnsupportedVersion);
    }
    let at_ns = clock.nanoseconds(header.timestamp)?;
    let body = match header.provider {
        Provider::Disk => {
            let request = scalar(fields, Field::Irp, header.pointer_bytes, true)?;
            let direction = if matches!(header.opcode, 10 | 12) {
                Direction::Read
            } else {
                Direction::Write
            };
            if matches!(header.opcode, 12 | 13) {
                Body::Begin {
                    request,
                    tid: scalar(fields, Field::IssuingThreadId, 4, true)? as u32,
                    direction,
                }
            } else {
                Body::Complete {
                    request,
                    bytes: scalar(fields, Field::TransferSize, 4, false)? as u32,
                    direction,
                }
            }
        }
        Provider::Process => Body::Process {
            pid: scalar(fields, Field::ProcessId, 4, false)? as u32,
            phase: phase.unwrap(),
        },
        Provider::Thread => Body::Thread {
            pid: scalar(fields, Field::ProcessId, 4, false)? as u32,
            tid: scalar(fields, Field::ThreadId, 4, false)? as u32,
            phase: phase.unwrap(),
        },
        Provider::Other => return Ok(None),
    };
    Ok(Some(Event { at_ns, body }))
}

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use std::ptr;
    use windows_sys::Win32::System::Diagnostics::Etw::{
        TdhGetProperty, TdhGetPropertySize, EVENT_HEADER_FLAG_32_BIT_HEADER,
        EVENT_HEADER_FLAG_64_BIT_HEADER, EVENT_RECORD, PROPERTY_DATA_DESCRIPTOR,
    };
    struct Properties<'a>(&'a EVENT_RECORD);
    impl Fields for Properties<'_> {
        fn read(&mut self, field: Field) -> Result<Scalar, Error> {
            let name: Vec<u16> = field.name().encode_utf16().chain(Some(0)).collect();
            let descriptor = PROPERTY_DATA_DESCRIPTOR {
                PropertyName: name.as_ptr() as u64,
                ArrayIndex: u32::MAX,
                Reserved: 0,
            };
            let mut size = 0;
            let code =
                unsafe { TdhGetPropertySize(self.0, 0, ptr::null(), 1, &descriptor, &mut size) };
            if code != 0 {
                return Err(Error::Native(code));
            }
            if !matches!(size, 4 | 8) {
                return Err(Error::InvalidWidth);
            }
            let mut bytes = [0u8; 8];
            let code = unsafe {
                TdhGetProperty(
                    self.0,
                    0,
                    ptr::null(),
                    1,
                    &descriptor,
                    size,
                    bytes.as_mut_ptr(),
                )
            };
            if code != 0 {
                return Err(Error::Native(code));
            }
            Ok(Scalar {
                value: u64::from_le_bytes(bytes),
                bytes: size as usize,
            })
        }
    }
    /// Decode only while the ETW callback owns valid EVENT_RECORD backing buffers.
    /// # Safety
    /// All pointers inside `record` must refer to the OS-provided buffers for this live
    /// callback; the session must use raw QPC timestamps matching `clock`.
    pub unsafe fn decode_record(
        record: &EVENT_RECORD,
        clock: &QpcClock,
    ) -> Result<Option<Event>, Error> {
        let guid = record.EventHeader.ProviderId;
        let family = guid.data2 == 0xfe05
            && guid.data3 == 0x11d0
            && guid.data4 == [0x9d, 0xda, 0, 0xc0, 0x4f, 0xd7, 0xba, 0x7c];
        let provider = if family {
            match guid.data1 {
                0x3d6fa8d4 => Provider::Disk,
                0x3d6fa8d0 => Provider::Process,
                0x3d6fa8d1 => Provider::Thread,
                _ => Provider::Other,
            }
        } else {
            Provider::Other
        };
        let flags = u32::from(record.EventHeader.Flags);
        let width = match (
            flags & EVENT_HEADER_FLAG_32_BIT_HEADER != 0,
            flags & EVENT_HEADER_FLAG_64_BIT_HEADER != 0,
        ) {
            (true, false) => 4,
            (false, true) => 8,
            _ => 0,
        };
        decode(
            Header {
                provider,
                opcode: record.EventHeader.EventDescriptor.Opcode,
                version: record.EventHeader.EventDescriptor.Version,
                pointer_bytes: width,
                timestamp: record.EventHeader.TimeStamp,
            },
            &mut Properties(record),
            clock,
        )
    }
}
#[cfg(target_os = "windows")]
pub use native::decode_record;

#[cfg(test)]
mod tests {
    use super::*;
    struct Mock(Vec<(Field, Scalar)>);
    impl Fields for Mock {
        fn read(&mut self, field: Field) -> Result<Scalar, Error> {
            self.0
                .iter()
                .find(|(f, _)| *f == field)
                .map(|(_, v)| *v)
                .ok_or(Error::MissingProperty)
        }
    }
    fn field(f: Field, value: u64, bytes: usize) -> (Field, Scalar) {
        (f, Scalar { value, bytes })
    }
    fn header(opcode: u8, width: usize) -> Header {
        Header {
            provider: Provider::Disk,
            opcode,
            version: 3,
            pointer_bytes: width,
            timestamp: 10,
        }
    }
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "explicit TDH decode of a synthetic fixture; no trace session or elevation"]
    fn native_tdh_decodes_synthetic_disk_completion() {
        use windows_sys::{core::GUID, Win32::System::Diagnostics::Etw::*};
        #[repr(C)]
        struct Payload {
            disk: u32,
            flags: u32,
            bytes: u32,
            reserved: u32,
            offset: u64,
            file: u64,
            irp: u64,
            response: u64,
            tid: u32,
            pad: u32,
        }
        let mut payload = Payload {
            disk: 0,
            flags: 0,
            bytes: 8192,
            reserved: 0,
            offset: 0,
            file: 0,
            irp: 0x12345678abcdef01,
            response: 0,
            tid: 20,
            pad: 0,
        };
        let mut record = EVENT_RECORD::default();
        record.EventHeader.Size = std::mem::size_of_val(&record.EventHeader) as u16;
        record.EventHeader.Flags =
            (EVENT_HEADER_FLAG_64_BIT_HEADER | EVENT_HEADER_FLAG_CLASSIC_HEADER) as u16;
        record.EventHeader.ProviderId = GUID {
            data1: 0x3d6fa8d4,
            data2: 0xfe05,
            data3: 0x11d0,
            data4: [0x9d, 0xda, 0, 0xc0, 0x4f, 0xd7, 0xba, 0x7c],
        };
        record.EventHeader.EventDescriptor.Opcode = 10;
        record.EventHeader.EventDescriptor.Version = 3;
        record.EventHeader.TimeStamp = 100;
        record.UserDataLength = std::mem::size_of_val(&payload) as u16;
        record.UserData = (&mut payload as *mut Payload).cast();
        let event = unsafe { decode_record(&record, &QpcClock::new(0, 1000).unwrap()) }
            .unwrap()
            .unwrap();
        assert_eq!(
            event.body,
            Body::Complete {
                request: payload.irp,
                bytes: 8192,
                direction: Direction::Read
            }
        );
    }

    #[test]
    fn pointers_are_decoded_by_event_width_and_completion_pid_is_not_used() {
        let clock = QpcClock::new(0, 1000).unwrap();
        for width in [4, 8] {
            let request = if width == 8 { u32::MAX as u64 + 7 } else { 9 };
            let begin = decode(
                header(12, width),
                &mut Mock(vec![
                    field(Field::Irp, request, width),
                    field(Field::IssuingThreadId, 20, 4),
                ]),
                &clock,
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                begin.body,
                Body::Begin {
                    request,
                    tid: 20,
                    direction: Direction::Read
                }
            );
            let end = decode(
                header(11, width),
                &mut Mock(vec![
                    field(Field::Irp, request, width),
                    field(Field::TransferSize, 123, 4),
                ]),
                &clock,
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                end.body,
                Body::Complete {
                    request,
                    bytes: 123,
                    direction: Direction::Write
                }
            );
        }
    }
    #[test]
    fn rundown_end_is_not_process_or_thread_termination() {
        let clock = QpcClock::new(0, 1000).unwrap();
        for provider in [Provider::Process, Provider::Thread] {
            let h = Header {
                provider,
                opcode: 4,
                version: 2,
                pointer_bytes: 8,
                timestamp: 10,
            };
            let e = decode(
                h,
                &mut Mock(vec![
                    field(Field::ProcessId, 7, 4),
                    field(Field::ThreadId, 8, 4),
                ]),
                &clock,
            )
            .unwrap()
            .unwrap();
            assert!(matches!(
                e.body,
                Body::Process {
                    phase: Lifecycle::PresentAtEnd,
                    ..
                } | Body::Thread {
                    phase: Lifecycle::PresentAtEnd,
                    ..
                }
            ));
        }
    }
    #[test]
    fn unsupported_schemas_missing_fields_and_widths_fail_closed() {
        let clock = QpcClock::new(0, 1000).unwrap();
        let mut h = header(12, 8);
        h.version = 99;
        assert_eq!(
            decode(h, &mut Mock(vec![]), &clock).unwrap_err(),
            Error::UnsupportedVersion
        );
        assert_eq!(
            decode(header(12, 8), &mut Mock(vec![]), &clock).unwrap_err(),
            Error::MissingProperty
        );
        assert_eq!(
            decode(
                header(12, 8),
                &mut Mock(vec![field(Field::Irp, 7, 4)]),
                &clock
            )
            .unwrap_err(),
            Error::InvalidWidth
        );
        assert!(decode(header(15, 8), &mut Mock(vec![]), &clock)
            .unwrap()
            .is_none());
    }
    #[test]
    fn qpc_conversion_rejects_negative_and_overflow() {
        assert!(QpcClock::new(0, 0).is_err());
        let c = QpcClock::new(100, 10_000_000).unwrap();
        assert_eq!(c.nanoseconds(10_000_100), Ok(1_000_000_000));
        assert!(c.nanoseconds(99).is_err());
        assert!(QpcClock::new(0, 1).unwrap().nanoseconds(i64::MAX).is_err());
    }
    #[test]
    fn normalized_disk_events_feed_the_production_accumulator() {
        use crate::enhanced::disk_io::{Limits, ProcessKey};
        let clock = QpcClock::new(0, 1000).unwrap();
        let mut a = Accumulator::new(0, Limits::default());
        let process = ProcessKey {
            pid: 5,
            creation: 9,
        };
        a.process_start(0, process);
        a.thread_start(0, 20, process);
        a.baseline_complete(0);
        let begin = decode(
            header(12, 8),
            &mut Mock(vec![
                field(Field::Irp, 9, 8),
                field(Field::IssuingThreadId, 20, 4),
            ]),
            &clock,
        )
        .unwrap()
        .unwrap();
        begin.apply_disk(&mut a);
        let mut h = header(10, 8);
        h.timestamp = 20;
        let end = decode(
            h,
            &mut Mock(vec![
                field(Field::Irp, 9, 8),
                field(Field::TransferSize, 4096, 4),
            ]),
            &clock,
        )
        .unwrap()
        .unwrap();
        end.apply_disk(&mut a);
        assert_eq!(a.window(1_000_000_000).rows[0].read_bps, Some(4096.0));
    }
}
