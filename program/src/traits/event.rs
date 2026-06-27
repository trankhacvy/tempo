use alloc::vec::Vec;

/// Anchor-compatible event instruction tag: Sha256(anchor:event)[..8].
///
/// State-changing instructions emit events via self-CPI through `EmitEvent`
/// (disc 228); this tag prefixes each event payload so indexers can decode it
/// (see `program/src/events/` and `utils::emit_event`).
pub const EVENT_IX_TAG: u64 = 0x1d9acb512ea545e4;
pub const EVENT_IX_TAG_LE: &[u8] = EVENT_IX_TAG.to_le_bytes().as_slice();

/// Length of event discriminator bytes (EVENT_IX_TAG_LE + discriminator byte)
pub const EVENT_DISCRIMINATOR_LEN: usize = 8 + 1;

/// Event discriminator values for this program.
///
/// Reserved for the events Tempo will emit once CPI event emission is wired up
/// These mirror the lifecycle of an auction.
#[repr(u8)]
pub enum EventDiscriminators {
    MarketInitialized = 0,
    OrderSubmitted = 1,
    OrderCancelled = 2,
    ChunkProcessed = 3,
    ClearingFinalized = 4,
    FillSettled = 5,
    OraclePriceRead = 6,
    FundingUpdated = 7,
    PositionLiquidated = 8,
}

/// Event discriminator with Anchor-compatible prefix
pub trait EventDiscriminator {
    /// Event discriminator byte
    const DISCRIMINATOR: u8;

    /// Full discriminator bytes including EVENT_IX_TAG_LE prefix
    #[inline(always)]
    fn discriminator_bytes() -> Vec<u8> {
        let mut data = Vec::with_capacity(EVENT_DISCRIMINATOR_LEN);
        data.extend_from_slice(EVENT_IX_TAG_LE);
        data.push(Self::DISCRIMINATOR);
        data
    }
}

/// Event serialization
pub trait EventSerialize: EventDiscriminator {
    /// Serialize event data (without discriminator)
    fn to_bytes_inner(&self) -> Vec<u8>;

    /// Serialize with full discriminator prefix
    #[inline(always)]
    fn to_bytes(&self) -> Vec<u8> {
        let mut data = Self::discriminator_bytes();
        data.extend_from_slice(&self.to_bytes_inner());
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestEvent;

    impl EventDiscriminator for TestEvent {
        const DISCRIMINATOR: u8 = 42;
    }

    #[test]
    fn test_discriminator_bytes_length() {
        let bytes = TestEvent::discriminator_bytes();
        assert_eq!(bytes.len(), EVENT_DISCRIMINATOR_LEN);
    }

    #[test]
    fn test_discriminator_bytes_prefix() {
        let bytes = TestEvent::discriminator_bytes();
        assert_eq!(&bytes[..8], EVENT_IX_TAG_LE);
        assert_eq!(bytes[8], 42);
    }

    #[test]
    fn test_event_discriminators_stable() {
        assert_eq!(EventDiscriminators::MarketInitialized as u8, 0);
        assert_eq!(EventDiscriminators::FillSettled as u8, 5);
    }
}
