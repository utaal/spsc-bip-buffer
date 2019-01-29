use std::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

struct BipBuffer {
    buf: *mut u8,
    len: usize,
    read: AtomicUsize,
    write: AtomicUsize,
    last: AtomicUsize,
}

pub struct BipBufferWriter {
    buffer: Arc<BipBuffer>,
    write: usize,
}

unsafe impl Send for BipBufferWriter {}

pub struct BipBufferReader {
    buffer: Arc<BipBuffer>,
    read: usize,
    priv_write: usize,
    last: usize,
}

unsafe impl Send for BipBufferReader {}

pub fn bip_buffer(mut buf_box: Box<[u8]>) -> (BipBufferWriter, BipBufferReader) {
    let len = buf_box.len();
    let buf = buf_box.as_mut_ptr();
    Box::into_raw(buf_box);
    let buffer = Arc::new(BipBuffer {
        buf,
        len,
        read: AtomicUsize::new(0),
        write: AtomicUsize::new(0),
        last: AtomicUsize::new(0),
    });

    (
        BipBufferWriter {
            buffer: buffer.clone(),
            write: 0,
        },
        BipBufferReader {
            buffer,
            read: 0,
            priv_write: 0,
            last: len,
        },
    )
}

#[derive(Clone, Copy)]
struct PendingReservation {
    start: usize,
    len: usize,
    wraparound: bool,
}

impl BipBufferWriter {
    fn reserve_core(&mut self, len: usize) -> Option<PendingReservation> {
        assert!(len > 0);
        let read = self.buffer.read.load(Ordering::Acquire);
        // eprintln!("try reserve: read:{:?} write:{:?}", read, self.write);
        if self.write >= read {
            if self.buffer.len.saturating_sub(self.write) >= len {
                // eprintln!("reserved {} at end (from {})", len, self.write);
                Some(PendingReservation {
                    start: self.write,
                    len,
                    wraparound: false,
                })
            } else {
                if read.saturating_sub(1) >= len {
                    // eprintln!("reserved {} at beginning (from {})", len, self.write);
                    Some(PendingReservation {
                        start: 0,
                        len,
                        wraparound: true,
                    })
                } else {
                    None
                }
            }
        } else {
            if (read - self.write).saturating_sub(1) >= len {
                // eprintln!("reserved {} at end (from {})", len, self.write);
                Some(PendingReservation {
                    start: self.write,
                    len,
                    wraparound: false,
                })
            } else {
                None
            }
        }
    }

    pub fn reserve(&mut self, len: usize) -> Option<BipBufferWriterReservation<'_>> {
        let reserved = self.reserve_core(len);
        if let Some(PendingReservation { start, len, wraparound }) = reserved {
            Some(BipBufferWriterReservation { writer: self, start, len, wraparound })
        } else {
            None
        }
    }

    pub fn spin_reserve(&mut self, len: usize) -> BipBufferWriterReservation<'_> {
        assert!(len <= self.buffer.len);
        let PendingReservation { start, len, wraparound } = loop {
            match self.reserve_core(len) {
                None => continue,
                Some(r) => break r,
            }
        };
        BipBufferWriterReservation { writer: self, start, len, wraparound }
    }
}

pub struct BipBufferWriterReservation<'a> {
    writer: &'a mut BipBufferWriter,
    start: usize,
    len: usize,
    wraparound: bool,
}

impl<'a> core::ops::Deref for BipBufferWriterReservation<'a> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self.writer.buffer.buf.add(self.start), self.len)
        }
    }
}

impl<'a> core::ops::DerefMut for BipBufferWriterReservation<'a> {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(self.writer.buffer.buf.add(self.start), self.len)
        }
    }
}

impl<'a> core::ops::Drop for BipBufferWriterReservation<'a> {
    fn drop(&mut self) {
        if self.wraparound {
            // eprintln!("writing last {}", self.writer.write);
            self.writer.buffer.last.store(self.writer.write, Ordering::Relaxed);
            self.writer.write = 0;
        }
        self.writer.write += self.len;
        self.writer.buffer.write.store(self.writer.write, Ordering::Release);
        // eprintln!("drop write:{} {:?}", self.writer.write, unsafe {
        // core::slice::from_raw_parts(self.writer.buffer.buf, self.writer.buffer.len) });
    }
}

impl<'a> BipBufferWriterReservation<'a> {
    pub fn send(self) {
        // drop
    }
}

impl BipBufferReader {
    pub fn valid(&mut self) -> &mut [u8] {
        self.priv_write = self.buffer.write.load(Ordering::Acquire);
        // eprintln!("valid (write):{:?} read:{:?} last:{:?}", self.priv_write, self.read, self.last);
        if self.priv_write >= self.read {
            unsafe {
                // eprintln!("slice (1) {} {}", self.read, self.priv_write);
                core::slice::from_raw_parts_mut(self.buffer.buf.add(self.read), self.priv_write - self.read)
            }
        } else {
            self.last = self.buffer.last.load(Ordering::Relaxed);
            if self.read == self.last {
                // eprintln!("moving last (v)");
                self.read = 0;
                self.last = self.buffer.len;
                self.buffer.last.store(self.last, Ordering::Relaxed);
                return self.valid();
            }
            unsafe {
                // eprintln!("slice (2) {} {}", self.last, self.read);
                core::slice::from_raw_parts_mut(self.buffer.buf.add(self.read), self.last - self.read)
            }
        }
    }

    pub fn consume(&mut self, len: usize) -> bool {
        // eprintln!("consume (write):{:?} read:{:?}", self.priv_write, self.read);
        if self.priv_write >= self.read {
            if len <= self.priv_write - self.read {
                self.read += len;
            } else {
                return false;
            }
        } else {
            let remaining = self.last - self.read;
            if len == remaining {
                // eprintln!("moving last (c)");
                self.read = 0;
                self.last = self.buffer.len;
                self.buffer.last.store(self.last, Ordering::Relaxed);
            } else if len <= remaining {
                self.read += len;
            } else {
                return false;
            }
        }
        self.buffer.read.store(self.read, Ordering::Release);
        // eprintln!("consumed (write):{:?} read:{:?} last:{:?}", self.priv_write, self.read, self.last);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::bip_buffer;

    #[test]
    fn basic() {
        for i in 0..128 {
            let (mut writer, mut reader) = bip_buffer(vec![0u8; 16].into_boxed_slice());
            let sender = std::thread::spawn(move || {
                writer.reserve(8).as_mut().expect("reserve").copy_from_slice(&[10, 11, 12, 13, 14, 15, 16, i]);
            });
            let receiver = std::thread::spawn(move || {
                while reader.valid().len() < 8 {}
                assert_eq!(reader.valid(), &[10, 11, 12, 13, 14, 15, 16, i]);
            });
            sender.join().unwrap();
            receiver.join().unwrap();
        }
    }

    #[test]
    fn static_prime_length() {
        const MSG_LENGTH: u8 = 17; // intentionally prime
        let (mut writer, mut reader) = bip_buffer(vec![128u8; 64].into_boxed_slice());
        let sender = std::thread::spawn(move || {
            let mut msg = [0u8; MSG_LENGTH as usize];
            for _ in 0..1024 {
                for i in 0..128u8 {
                    &mut msg[..].copy_from_slice(&[i; MSG_LENGTH as usize][..]);
                    msg[i as usize % (MSG_LENGTH as usize)] = 0;
                    // eprintln!(">>>>>>> {} {:?}", i, msg);
                    writer.spin_reserve(MSG_LENGTH as usize).copy_from_slice(&msg[..]);
                }
            }
        });
        let receiver = std::thread::spawn(move || {
            let mut msg = [0u8; MSG_LENGTH as usize];
            for _ in 0..1024 {
                for i in 0..128u8 {
                    // eprintln!("<<<<<<< {}", i);
                    &mut msg[..].copy_from_slice(&[i; MSG_LENGTH as usize][..]);
                    msg[i as usize % (MSG_LENGTH as usize)] = 0;
                    while reader.valid().len() < (MSG_LENGTH as usize) {}
                    // eprintln!("<<<<<<< checking {} {:?}", i, msg);
                    assert_eq!(&reader.valid()[..MSG_LENGTH as usize], &msg[..]);
                    assert!(reader.consume(MSG_LENGTH as usize));
                    // eprintln!("<<<<<<< ok {}", i);
                }
            }
        });
        sender.join().unwrap();
        receiver.join().unwrap();
    }

    #[test]
    fn random_length() {
        use rand::Rng;

        const MAX_LENGTH: usize = 127;
        let (mut writer, mut reader) = bip_buffer(vec![0u8; 1024].into_boxed_slice());
        let sender = std::thread::spawn(move || {
            let mut rng = rand::thread_rng();
            let mut msg = [0u8; MAX_LENGTH];
            for _ in 0..1024 {
                for round in 0..128u8 {
                    let length: u8 = rng.gen_range(1, MAX_LENGTH as u8);
                    msg[0] = length;
                    for i in 1..length {
                        msg[i as usize] = round;
                    }
                    writer.spin_reserve(length as usize).copy_from_slice(&msg[..length as usize]);
                }
            }
        });
        let receiver = std::thread::spawn(move || {
            let mut msg = [0u8; MAX_LENGTH];
            for _ in 0..1024 {
                for round in 0..128u8 {
                    let msg_len = loop {
                        let valid = reader.valid();
                        if valid.len() < 1 { continue; }
                        break valid[0] as usize;
                    };
                    let recv_msg = loop {
                        let valid = reader.valid();
                        if valid.len() < msg_len { continue; }
                        break valid;
                    };
                    msg[0] = msg_len as u8;
                    for i in 1..msg_len {
                        msg[i as usize] = round;
                    }
                    assert_eq!(&recv_msg[..msg_len], &msg[..msg_len]);
                    assert!(reader.consume(msg_len as usize));
                }
            }
        });
        sender.join().unwrap();
        receiver.join().unwrap();
    }

}
