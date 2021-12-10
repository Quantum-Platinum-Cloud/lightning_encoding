// LNP/BP Core Library implementing LNPBP specifications & standards
// Written in 2020 by
//     Dr. Maxim Orlovsky <orlovsky@pandoracore.com>
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the MIT License
// along with this software.
// If not, see <https://opensource.org/licenses/MIT>.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::hash::Hash;

use bitcoin::util::psbt::PartiallySignedTransaction as Psbt;
use bitcoin::{OutPoint, Transaction, TxIn, TxOut};
use lnp2p::legacy::Messages;
use strict_encoding::{StrictDecode, StrictEncode};

use super::extension::{self, ChannelExtension, Extension};

#[derive(
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Debug,
    Display,
    Error,
    From,
    StrictEncode,
    StrictDecode,
)]
#[display(doc_comments)]
pub enum Error {
    /// Extension-specific error: {0}
    Extension(String),

    /// HTLC extension error
    // TODO: Expand into specific error types
    #[display(inner)]
    Htlc(String),
}

/// Marker trait for any data that can be used as a part of the channel state
pub trait State {}

// Allow empty state
impl State for () {}

/// Channel state is a sum of the state from all its extensions
pub type IntegralState<N> = BTreeMap<N, Box<dyn State>>;
impl<N> State for IntegralState<N> where N: extension::Nomenclature {}

pub type ExtensionQueue<N> =
    BTreeMap<N, Box<dyn ChannelExtension<Identity = N>>>;

/// Channel operates as a three sets of extensions, where each set is applied
/// to construct the transaction graph and the state in a strict order one after
/// other. The order of the extensions within each set is defined by the
/// concrete type implementing `extension::Nomenclature` marker trait, provided
/// as a type parameter `N`
#[derive(Getters)]
pub struct Channel<N>
where
    N: extension::Nomenclature,
{
    /// Constructor extensions constructs base transaction graph. There could
    /// be only a single extension of this type
    constructor: Box<dyn ChannelExtension<Identity = N>>,

    /// Extender extensions adds additional outputs to the transaction graph
    /// and the state data associated with these outputs, like HTLCs, PTLCs,
    /// anchored outputs, DLC-specific outs etc
    extenders: ExtensionQueue<N>,

    /// Modifier extensions do not change number of outputs, but may change
    /// their ordering or tweak individual inputs, outputs and public keys.
    /// These extensions may include: BIP96 lexicographic ordering, RGB, Liquid
    modifiers: ExtensionQueue<N>,
}

impl<N> Channel<N>
where
    N: extension::Nomenclature,
{
    pub fn with(
        constructor: Box<dyn ChannelExtension<Identity = N>>,
        extenders: impl IntoIterator<Item = Box<dyn ChannelExtension<Identity = N>>>,
        modifiers: impl IntoIterator<Item = Box<dyn ChannelExtension<Identity = N>>>,
    ) -> Self {
        Self {
            constructor,
            extenders: extenders.into_iter().fold(
                ExtensionQueue::<N>::new(),
                |mut queue, e| {
                    queue.insert(e.identity(), e);
                    queue
                },
            ),
            modifiers: modifiers.into_iter().fold(
                ExtensionQueue::<N>::new(),
                |mut queue, e| {
                    queue.insert(e.identity(), e);
                    queue
                },
            ),
        }
    }

    #[inline]
    pub fn add_extension(
        &mut self,
        extension: Box<dyn ChannelExtension<Identity = N>>,
    ) {
        self.extenders.insert(extension.identity(), extension);
    }

    #[inline]
    pub fn add_modifier(
        &mut self,
        modifier: Box<dyn ChannelExtension<Identity = N>>,
    ) {
        self.modifiers.insert(modifier.identity(), modifier);
    }
}

impl<N> Default for Channel<N>
where
    N: extension::Nomenclature,
{
    fn default() -> Self {
        Channel::with(
            N::default_constructor(),
            N::default_extenders(),
            N::default_modifiers(),
        )
    }
}

/// Channel is the extension to itself :) so it receives the same input as any
/// other extension and just forwards it to them
impl<N> Extension for Channel<N>
where
    N: 'static + extension::Nomenclature,
{
    type Identity = N;

    #[inline]
    fn new() -> Box<dyn ChannelExtension<Identity = Self::Identity>> {
        Box::new(Channel::default())
    }

    fn identity(&self) -> Self::Identity {
        N::default()
    }

    fn update_from_peer(&mut self, message: &Messages) -> Result<(), Error> {
        N::update_from_peer(self, message)?;
        self.constructor.update_from_peer(message)?;
        self.extenders
            .iter_mut()
            .try_for_each(|(_, e)| e.update_from_peer(message))?;
        self.modifiers
            .iter_mut()
            .try_for_each(|(_, e)| e.update_from_peer(message))?;
        Ok(())
    }

    fn extension_state(&self) -> Box<dyn State> {
        let mut data = IntegralState::<N>::new();
        data.insert(
            self.constructor.identity(),
            self.constructor.extension_state(),
        );
        self.extenders.iter().for_each(|(id, e)| {
            data.insert(*id, e.extension_state());
        });
        self.modifiers.iter().for_each(|(id, e)| {
            data.insert(*id, e.extension_state());
        });
        Box::new(data)
    }
}

/// Channel is the extension to itself :) so it receives the same input as any
/// other extension and just forwards it to them
impl<N> ChannelExtension for Channel<N>
where
    N: 'static + extension::Nomenclature,
{
    fn channel_state(&self) -> Box<dyn State> {
        let mut data = IntegralState::<N>::new();
        data.insert(
            self.constructor.identity(),
            self.constructor.extension_state(),
        );
        self.extenders.iter().for_each(|(id, e)| {
            data.insert(*id, e.extension_state());
        });
        self.modifiers.iter().for_each(|(id, e)| {
            data.insert(*id, e.extension_state());
        });
        Box::new(data)
    }

    fn apply(&mut self, tx_graph: &mut TxGraph) -> Result<(), Error> {
        self.constructor.apply(tx_graph)?;
        self.extenders
            .iter_mut()
            .try_for_each(|(_, e)| e.apply(tx_graph))?;
        self.modifiers
            .iter_mut()
            .try_for_each(|(_, e)| e.apply(tx_graph))?;
        Ok(())
    }
}

pub trait TxRole: Clone + From<u16> + Into<u16> {}
pub trait TxIndex: Clone + From<u64> + Into<u64> {}

impl TxRole for u16 {}
impl TxIndex for u64 {}

#[derive(Getters, Clone, PartialEq, StrictEncode, StrictDecode)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)]
pub struct TxGraph {
    funding_parties: u8,
    funding_threshold: u8,
    funding_tx: Psbt,
    funding_outpoint: OutPoint,
    commitment_outpoint: OutPoint, /* We should have a commitment outpoint
                                    * for HTLC success and timeout Tx */
    pub cmt_version: i32,
    pub cmt_locktime: u32,
    pub cmt_sequence: u32,
    pub cmt_outs: Vec<TxOut>,
    graph: BTreeMap<u16, BTreeMap<u64, Psbt>>,
}

impl TxGraph {
    pub fn tx<R, I>(&self, role: R, index: I) -> Option<&Psbt>
    where
        R: TxRole,
        I: TxIndex,
    {
        self.graph
            .get(&role.into())
            .and_then(|v| v.get(&index.into()))
    }

    pub fn tx_mut<R, I>(&mut self, role: R, index: I) -> Option<&mut Psbt>
    where
        R: TxRole,
        I: TxIndex,
    {
        self.graph
            .get_mut(&role.into())
            .and_then(|v| v.get_mut(&index.into()))
    }

    pub fn insert_tx<R, I>(
        &mut self,
        role: R,
        index: I,
        psbt: Psbt,
    ) -> Option<Psbt>
    where
        R: TxRole,
        I: TxIndex,
    {
        self.graph
            .entry(role.into())
            .or_insert(empty!())
            .insert(index.into(), psbt)
    }

    pub fn len(&self) -> usize {
        self.graph
            .iter()
            .fold(0usize, |sum, (_, map)| sum + map.len())
    }

    pub fn last_index<R>(&self, role: R) -> usize
    where
        R: TxRole,
    {
        match self.graph.get(&role.into()) {
            Some(map) => map.len(),
            None => 0usize,
        }
    }

    pub fn render(&self) -> Vec<Psbt> {
        let mut txes = Vec::with_capacity(self.len());
        let cmt_tx = self.render_cmt();
        txes.push(cmt_tx);
        txes.extend(self.graph.values().flat_map(|v| v.values().cloned()));
        txes
    }

    pub fn render_cmt(&self) -> Psbt {
        let cmt_tx = Transaction {
            version: self.cmt_version,
            lock_time: self.cmt_locktime,
            input: vec![TxIn {
                previous_output: self.funding_outpoint,
                script_sig: empty!(),
                sequence: self.cmt_sequence,
                witness: empty!(),
            }],
            output: self.cmt_outs.clone(),
        };
        Psbt::from_unsigned_tx(cmt_tx).expect(
            "PSBT construction fails only if script_sig and witness are not \
                empty; which is not the case here",
        )
    }

    pub fn iter(&self) -> GraphIter {
        GraphIter::with(self)
    }

    pub fn vec_mut(&mut self) -> Vec<(u16, u64, &mut Psbt)> {
        let vec = self
            .graph
            .iter_mut()
            .flat_map(|(role, map)| {
                map.iter_mut().map(move |(index, tx)| (*role, *index, tx))
            })
            .collect::<Vec<_>>();
        vec
    }
}

impl Default for TxGraph {
    fn default() -> Self {
        Self {
            funding_parties: 0,
            funding_threshold: 0,
            funding_tx: Psbt::from_unsigned_tx(Transaction {
                version: 2,
                lock_time: 0,
                input: none!(),
                output: none!(),
            })
            .expect(""),
            funding_outpoint: none!(),
            commitment_outpoint: none!(),
            cmt_version: 2,
            cmt_locktime: 0,
            cmt_sequence: 0,
            cmt_outs: none!(),
            graph: empty!(),
        }
    }
}

pub struct GraphIter<'a> {
    graph: &'a TxGraph,
    curr_role: u16,
    curr_index: u64,
}

impl<'a> GraphIter<'a> {
    fn with(graph: &'a TxGraph) -> Self {
        Self {
            graph,
            curr_role: 0,
            curr_index: 0,
        }
    }
}

impl<'a> Iterator for GraphIter<'a> {
    type Item = (u16, u64, &'a Psbt);

    fn next(&mut self) -> Option<Self::Item> {
        let tx = self.graph.tx(self.curr_role, self.curr_index).or_else(|| {
            self.curr_role += 1;
            self.curr_index = 0;
            self.graph.tx(self.curr_role, self.curr_index)
        });
        self.curr_index += 1;
        tx.map(|tx| (self.curr_role, self.curr_index, tx))
    }
}

pub trait History {
    type State;
    type Error: std::error::Error;

    fn height(&self) -> usize;
    fn get(&self, height: usize) -> Result<Self::State, Self::Error>;
    fn top(&self) -> Result<Self::State, Self::Error>;
    fn bottom(&self) -> Result<Self::State, Self::Error>;
    fn dig(&self) -> Result<Self::State, Self::Error>;
    fn push(&mut self, state: Self::State) -> Result<&mut Self, Self::Error>;
}
