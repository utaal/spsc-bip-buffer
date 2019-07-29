//! `spsc-bip-buffer` is a single-producer single-consumer circular buffer that always supports
//! writing a contiguous chunk of data. Write requests that cannot fit in an available contiguous
//! area will wait till space is newly available (after the consumer has read the data).
//!
//! `spsc-bip-buffer` is lock-free and uses atomics for coordination.
//!
//! # Example
//!
//! ```
//! use spsc_bip_buffer::bip_buffer_with_len;
//! let (mut writer, mut reader) = bip_buffer_with_len(256);
//! let sender = std::thread::spawn(move || {
//!     for i in 0..128 {
//!         let mut reservation = writer.spin_reserve(8);
//!         reservation.copy_from_slice(&[10, 11, 12, 13, 14, 15, 16, i]);
//!         reservation.send(); // optional, dropping has the same effect
//!     }
//! });
//! let receiver = std::thread::spawn(move || {
//!     for i in 0..128 {
//!         while reader.valid().len() < 8 {}
//!         assert_eq!(&reader.valid()[..8], &[10, 11, 12, 13, 14, 15, 16, i]);
//!         reader.consume(8);
//!     }
//! });
//! sender.join().unwrap();
//! receiver.join().unwrap();
//! ```

#![deny(missing_docs)]

use std::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use cache_line_size::CacheAligned;

struct BipBuffer<T> {
    sequestered: Box<dyn std::any::Any>,
    buf: *mut T,
    len: usize,
    read: CacheAligned<AtomicUsize>,
    write: CacheAligned<AtomicUsize>,
    last: CacheAligned<AtomicUsize>,
}

#[cfg(feature = "debug")]
impl BipBuffer<T> {
    fn dbg_info(&self) -> String {
        format!(" read: {:?} -- write: {:?} -- last: {:?}  [len: {:?}] ",
                self.read.0,
                self.write.0,
                self.last.0,
                self.len)
    }
}

/// Represents the send side of the single-producer single-consumer circular buffer.
///
/// `BipBufferWriter` is `Send` so you can move it to the sender thread.
pub struct BipBufferWriter<T> {
    buffer: Arc<BipBuffer<T>>,
    write: usize,
    last: usize,
}

unsafe impl<T> Send for BipBufferWriter<T> {}

/// Represents the receive side of the single-producer single-consumer circular buffer.
///
/// `BipBufferReader` is `Send` so you can move it to the receiver thread.
pub struct BipBufferReader<T> {
    buffer: Arc<BipBuffer<T>>,
    read: usize,
    priv_write: usize,
    priv_last: usize,
}

unsafe impl<T> Send for BipBufferReader<T> {}

/// Creates a new `BipBufferWriter`/`BipBufferReader` pair using the provided underlying storage.
///
/// `BipBufferWriter` and `BipBufferReader` represent the send and receive side of the
/// single-producer single-consumer circular buffer respectively.
///
/// ### Storage
///
/// This method takes ownership of the storage which can be recovered with `try_unwrap` on
/// `BipBufferWriter` or `BipBufferReader`. If both sides of the channel have been dropped
/// (not using `try_unwrap`), the storage is dropped.
pub fn bip_buffer_from<B, T>(from: B) -> (BipBufferWriter<T>, BipBufferReader<T>)
where
    B: std::ops::DerefMut<Target = [T]> + 'static,
{
    let mut sequestered = Box::new(from);
    let len = sequestered.len();
    let buf = sequestered.as_mut_ptr();

    let buffer = Arc::new(BipBuffer {
        sequestered,
        buf,
        len,
        read: CacheAligned(AtomicUsize::new(0)),
        write: CacheAligned(AtomicUsize::new(0)),
        last: CacheAligned(AtomicUsize::new(0)),
    });

    (
        BipBufferWriter {
            buffer: buffer.clone(),
            write: 0,
            last: len,
        },
        BipBufferReader {
            buffer,
            read: 0,
            priv_write: 0,
            priv_last: len,
        },
    )
}

/// Creates a new `BipBufferWriter`/`BipBufferReader` pair using a `Vec` of the specified length
/// as underlying storage.
///
/// `BipBufferWriter` and `BipBufferReader` represent the send and receive side of the
/// single-producer single-consumer queue respectively.
pub fn bip_buffer_with_len<T>(len: usize) -> (BipBufferWriter<T>, BipBufferReader<T>)
where
    T: Copy + Default + 'static,
{
    bip_buffer_from(vec![T::default(); len].into_boxed_slice())
}

impl<T> BipBuffer<T> {
    // NOTE: Panics if B is not the type of the underlying storage.
    fn into_inner<B>(self) -> B
    where
        B: std::ops::DerefMut<Target = [T]> + 'static,
    {
        let BipBuffer { sequestered, .. } = self;
        *sequestered.downcast::<B>().expect("incorrect underlying type")
    }
}

#[derive(Clone, Copy)]
struct PendingReservation {
    start: usize,
    len: usize,
    wraparound: bool,
}

impl<T> BipBufferWriter<T> {
    fn reserve_core(&mut self, len: usize) -> Option<PendingReservation> {
        assert!(len > 0);
        let read = self.buffer.read.0.load(Ordering::Acquire);
        if self.write >= read {
            if self.buffer.len.saturating_sub(self.write) >= len {
                Some(PendingReservation {
                    start: self.write,
                    len,
                    wraparound: false,
                })
            } else {
                if read.saturating_sub(1) >= len {
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

    /// Attempt to reserve the requested number of bytes. If no contiguous `len` bytes are available
    /// in the circular buffer, this method returns `None`: check `spin_reserve` for a method that
    /// busy waits till space is available.
    ///
    /// If successful, it returns a reservation that the sender can use to write data to the buffer.
    /// Dropping the reservation signals completion of the write and makes the data available to the
    /// reader.
    pub fn reserve(&mut self, len: usize) -> Option<BipBufferWriterReservation<'_, T>> {
        let reserved = self.reserve_core(len);
        if let Some(PendingReservation { start, len, wraparound }) = reserved {
            Some(BipBufferWriterReservation { writer: self, start, len, wraparound })
        } else {
            None
        }
    }

    /// Reserve the requested number of bytes. This method busy-waits until `len` contiguous bytes
    /// are available in the circular buffer.
    ///
    /// If successful, it returns a reservation that the sender can use to write data to the buffer.
    /// Dropping the reservation signals completion of the write and makes the data available to the
    /// reader.
    ///
    /// ### Busy-waiting
    ///
    /// If the current thread is scheduled on the same core as the receiver, busy-waiting may
    /// compete with the receiver [...]
    pub fn spin_reserve(&mut self, len: usize) -> BipBufferWriterReservation<'_, T> {
        assert!(len <= self.buffer.len);
        let PendingReservation { start, len, wraparound } = loop {
            match self.reserve_core(len) {
                None => continue,
                Some(r) => break r,
            }
        };
        BipBufferWriterReservation { writer: self, start, len, wraparound }
    }

    /// Attempts to recover the underlying storage. B must be the type of the storage passed to
    /// `bip_buffer_from`. If the `BipBufferReader` side still exists, this will fail and return
    /// `Err(self)`. If the `BipBufferReader` side was dropped, this will return the underlying
    /// storage.
    ///
    /// # Panic
    ///
    /// Panics if B is not the type of the underlying storage.
    pub fn try_unwrap<B>(self) -> Result<B, Self>
    where
        B: std::ops::DerefMut<Target = [T]> + 'static,
    {
        let BipBufferWriter { buffer, write, last, } = self;
        match Arc::try_unwrap(buffer) {
            Ok(b) => Ok(b.into_inner()),
            Err(buffer) => Err(BipBufferWriter { buffer, write, last, }),
        }
    }
}

/// A write reservation returned by `reserve` or `spin_reserve`. The sender can access the reserved
/// buffer slice by dererferencing this reservation. Its size is guaranteed to match the requested
/// length in `reserve` or `spin_reserve`.
///
/// There are no guarantees on the contents of the buffer slice when a new reservation is created:
/// the slice may contain stale data or garbage.
///
/// Dropping the reservation (or calling `send`, which consumes `self`) marks the end of the write
/// and informs the reader that the data in this slice can now be read.
///
/// Don't drop the reservation before you're done writing!
///
/// # Examples
/// ```
/// use spsc_bip_buffer::bip_buffer_from;
/// let (mut writer, _) = bip_buffer_from(vec![0u8; 1024]);
/// std::thread::spawn(move || {
///   let mut reservation = writer.spin_reserve(4);
///   reservation[0] = 0xf0;
///   reservation[1] = 0x0f;
///   reservation[2] = 0x00;
///   reservation[3] = 0xee;
///   reservation.send();
/// }).join().unwrap();
/// ```
///
/// ```
/// use spsc_bip_buffer::bip_buffer_from;
/// let (mut writer, _) = bip_buffer_from(vec![0u8; 1024]);
/// std::thread::spawn(move || {
///   let mut reservation = writer.spin_reserve(4);
///   let data = vec![0xf0, 0x0f, 0x00, 0xee];
///   reservation.copy_from_slice(&data[..]);
///   // drop reservation, which sends data
/// }).join().unwrap();
/// ```
pub struct BipBufferWriterReservation<'a, T> {
    writer: &'a mut BipBufferWriter<T>,
    start: usize,
    len: usize,
    wraparound: bool,
}

impl<'a, T> core::ops::Deref for BipBufferWriterReservation<'a, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { core::slice::from_raw_parts(self.writer.buffer.buf.add(self.start), self.len) }
    }
}

impl<'a, T> core::ops::DerefMut for BipBufferWriterReservation<'a, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { core::slice::from_raw_parts_mut(self.writer.buffer.buf.add(self.start), self.len) }
    }
}

impl<'a, T> core::ops::Drop for BipBufferWriterReservation<'a, T> {
    fn drop(&mut self) {
        if self.wraparound {
            self.writer.buffer.last.0.store(self.writer.write, Ordering::Relaxed);
            self.writer.write = 0;
        }
        self.writer.write += self.len;
        if self.writer.write > self.writer.last {
            self.writer.last = self.writer.write;
            self.writer.buffer.last.0.store(self.writer.last, Ordering::Relaxed);
        }
        self.writer.buffer.write.0.store(self.writer.write, Ordering::Release);

        #[cfg(feature = "debug")]
        eprintln!("+++{}", self.writer.buffer.dbg_info());
    }
}

impl<'a, T> BipBufferWriterReservation<'a, T> {
    /// Calling `send` (or simply dropping the reservation) marks the end of the write and informs
    /// the reader that the data in this slice can now be read.
    pub fn send(self) {
        // drop
    }
}

impl<T> BipBufferReader<T> {
    /// Returns a mutable reference to a slice that contains the data written by the writer and not
    /// yet consumed by the reader. This is the receiving end of the circular buffer.
    ///
    /// The caller is free to mutate the data in this slice.
    pub fn valid(&mut self) -> &mut [T] {
        #[cfg(feature = "debug")]
        eprintln!("???{}", self.buffer.dbg_info());
        self.priv_write = self.buffer.write.0.load(Ordering::Acquire);

        if self.priv_write >= self.read {
            unsafe {
                core::slice::from_raw_parts_mut(self.buffer.buf.add(self.read), self.priv_write - self.read)
            }
        } else {
            self.priv_last = self.buffer.last.0.load(Ordering::Relaxed);
            if self.read == self.priv_last {
                self.read = 0;
                return self.valid();
            }
            unsafe {
                core::slice::from_raw_parts_mut(self.buffer.buf.add(self.read), self.priv_last - self.read)
            }
        }
    }

    /// Consumes the first `len` bytes in `valid`. This marks them as read and they won't be
    /// included in the slice returned by the next invocation of `valid`. This is used to
    /// communicate the reader's progress and free buffer space for future writes.
    pub fn consume(&mut self, len: usize) -> bool {
        if self.priv_write >= self.read {
            if len <= self.priv_write - self.read {
                self.read += len;
            } else {
                return false;
            }
        } else {
            let remaining = self.priv_last - self.read;
            if len == remaining {
                self.read = 0;
            } else if len <= remaining {
                self.read += len;
            } else {
                return false;
            }
        }
        self.buffer.read.0.store(self.read, Ordering::Release);
        #[cfg(feature = "debug")]
        eprintln!("---{}", self.buffer.dbg_info());
        true
    }

    /// Attempts to recover the underlying storage. B must be the type of the storage passed to
    /// `bip_buffer_from`. If the `BipBufferWriter` side still exists, this will fail and return
    /// `Err(self)`. If the `BipBufferWriter` side was dropped, this will return the underlying
    /// storage.
    ///
    /// # Panic
    ///
    /// Panics if B is not the type of the underlying storage.
    pub fn try_unwrap<B>(self) -> Result<B, Self>
    where
        B: std::ops::DerefMut<Target = [T]> + 'static,
    {
        let BipBufferReader { buffer, read, priv_write, priv_last, } = self;
        match Arc::try_unwrap(buffer) {
            Ok(b) => Ok(b.into_inner()),
            Err(buffer) => Err(BipBufferReader { buffer, read, priv_write, priv_last, }),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bip_buffer_from;

    #[test]
    fn basic() {
        for i in 0..128 {
            let (mut writer, mut reader) = bip_buffer_from(vec![0u8; 16].into_boxed_slice());
            let sender = std::thread::spawn(move || {
                writer.reserve(8).as_mut().expect("reserve").copy_from_slice(&[10, 11, 12, 13, 14, 15, 16, i]);
            });
            let receiver = std::thread::spawn(move || {
                while reader.valid().len() < 8 {}
                assert_eq!(reader.valid(), &[10, 11, 12, 13, 14, 15, 16, i]);
                reader.consume(8);
            });
            sender.join().unwrap();
            receiver.join().unwrap();
        }
    }

    #[test]
    fn spsc() {
        let (mut writer, mut reader) = bip_buffer_from(vec![0u8; 256].into_boxed_slice());
        let sender = std::thread::spawn(move || {
            for i in 0..128 {
                writer.spin_reserve(8).copy_from_slice(&[10, 11, 12, 13, 14, 15, 16, i]);
            }
        });
        let receiver = std::thread::spawn(move || {
            for i in 0..128 {
                while reader.valid().len() < 8 {}
                assert_eq!(&reader.valid()[..8], &[10, 11, 12, 13, 14, 15, 16, i]);
                reader.consume(8);
            }
        });
        sender.join().unwrap();
        receiver.join().unwrap();
    }

    #[test]
    fn provided_storage() {
        let storage = vec![0u8; 256].into_boxed_slice();
        let (mut writer, mut reader) = bip_buffer_from(storage);
        let sender = std::thread::spawn(move || {
            writer.spin_reserve(8).copy_from_slice(&[10, 11, 12, 13, 14, 15, 16, 17]);
        });
        let receiver = std::thread::spawn(move || {
            while reader.valid().len() < 8 {}
            reader.consume(8);
            reader
        });
        sender.join().unwrap();
        let reader = receiver.join().unwrap();
        let _: Box<[u8]> = reader.try_unwrap().map_err(|_| ()).expect("failed to recover storage");
    }

    #[test]
    #[should_panic]
    fn provided_storage_wrong_type() {
        let storage = vec![0u8; 256].into_boxed_slice();
        let (writer, reader) = bip_buffer_from(storage);
        std::mem::drop(writer);
        let _: Vec<u8> = reader.try_unwrap().map_err(|_| ()).expect("failed to recover storage");
    }

    #[test]
    fn provided_storage_still_alive() {
        let storage = vec![0u8; 256].into_boxed_slice();
        let (writer, reader) = bip_buffer_from(storage);
        let result: Result<Box<[u8]>, _> = reader.try_unwrap();
        assert!(result.is_err());
        std::mem::drop(writer);
    }

    #[test]
    fn static_prime_length() {
        const MSG_LENGTH: u8 = 17; // intentionally prime
        let (mut writer, mut reader) = bip_buffer_from(vec![128u8; 64].into_boxed_slice());
        let sender = std::thread::spawn(move || {
            let mut msg = [0u8; MSG_LENGTH as usize];
            for _ in 0..1024 {
                for i in 0..128u8 {
                    &mut msg[..].copy_from_slice(&[i; MSG_LENGTH as usize][..]);
                    msg[i as usize % (MSG_LENGTH as usize)] = 0;
                    writer.spin_reserve(MSG_LENGTH as usize).copy_from_slice(&msg[..]);
                }
            }
        });
        let receiver = std::thread::spawn(move || {
            let mut msg = [0u8; MSG_LENGTH as usize];
            for _ in 0..1024 {
                for i in 0..128u8 {
                    &mut msg[..].copy_from_slice(&[i; MSG_LENGTH as usize][..]);
                    msg[i as usize % (MSG_LENGTH as usize)] = 0;
                    while reader.valid().len() < (MSG_LENGTH as usize) {}
                    assert_eq!(&reader.valid()[..MSG_LENGTH as usize], &msg[..]);
                    assert!(reader.consume(MSG_LENGTH as usize));
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
        let (mut writer, mut reader) = bip_buffer_from(vec![0u8; 1024]);
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
