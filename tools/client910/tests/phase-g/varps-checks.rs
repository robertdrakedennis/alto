#[allow(unused_imports)]
use crate::{entities910, protocol910};
use entities910::varps::Varps;
use protocol910::{
    live::{Contexts, Feed},
    varbits::{Binding, Type},
    varp, Error,
};
fn bit() -> Type {
    let mut b = Type::empty(0);
    b.domain = Some(0);
    b.base_id = 1;
    b.binding = Some(Binding::empty(0, 1));
    b.start = 0;
    b.end = 0;
    b
}
#[test]
fn malformed_packets_preserve_arrays_and_pending_hash_state() {
    let mut prior = Varps::new(2);
    prior.set_local(1, 17, 100).unwrap();
    for (op, valid) in [
        (50, vec![1, 0, 0, 0, 0, 3]),
        (157, vec![5, 0, 129]),
        (44, vec![127, 0, 128]),
        (142, vec![0, 128, 1, 0, 0, 0]),
        (99, vec![]),
    ] {
        for n in 0..valid.len() {
            assert!(varp::decode(op, &valid[..n], &prior, 300, &|_| Ok(bit())).is_err());
        }
        let mut extra = valid.clone();
        extra.push(0);
        assert!(varp::decode(op, &extra, &prior, 300, &|_| Ok(bit())).is_err());
        let before = prior.clone();
        varp::decode(op, &valid, &prior, 300, &|_| Ok(bit())).unwrap();
        assert_eq!(prior, before);
    }
}
#[test]
fn varp_front_cannot_be_overtaken_by_entity_packets() {
    let mut f = Feed::default();
    f.state.varps = Some(Varps::new(2));
    assert!(f.enqueue(44, &[127, 0, 128]));
    assert!(f.enqueue(129, &[]));
    let before = f.state.clone();
    assert!(f
        .apply_varp_next(1, &|_| Err(Error::UnsupportedContext("missing binding")))
        .is_err());
    assert_eq!(f.state, before);
    assert_eq!(f.pending_len(), 2);
    assert_eq!(f.drain_completed().count(), 0);
    let a = f.apply_varp_next(1, &|_| Ok(bit())).unwrap().unwrap();
    assert_eq!(a.bytes, 3);
    assert!(!a.varp.unwrap().ignored_overflow);
    assert!(
        f.apply_next(&Contexts {
            rebuild: None,
            player: None,
            npc: None,
            zone: None
        })
        .unwrap()
        .unwrap()
        .read_batch_end
    );
    assert_eq!(f.pending_len(), 0);
    assert_eq!(f.drain_completed().count(), 2);
}
#[test]
fn intentional_server_overflow_has_an_explicit_receipt() {
    let prior = Varps::new(2);
    let mut state = prior.clone();
    let a = varp::apply(44, &[126, 0, 128], &mut state, 100, &|_| Ok(bit())).unwrap();
    assert_eq!(state, prior);
    assert_eq!(a.consumed, 3);
    assert!(a.outcome.ignored_overflow);
    assert_eq!(a.outcome.clock_reads, 0);
    let a = varp::apply(99, &[], &mut state, 100, &|_| panic!()).unwrap();
    assert_eq!(a.transmit_increment, 64);
}
