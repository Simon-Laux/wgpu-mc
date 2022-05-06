use crate::mc::block::{BlockPos, BlockState};
use std::collections::HashMap;

use crate::render::world::chunk::BakedChunkLayer;

use arc_swap::ArcSwap;

use parking_lot::RwLock;
use rayon::iter::IntoParallelRefIterator;
use std::convert::TryInto;
use std::sync::Arc;
use std::time::Instant;

use crate::mc::BlockManager;
use crate::render::pipeline::grass::GrassVertex;
use crate::render::pipeline::terrain::TerrainVertex;

pub const CHUNK_WIDTH: usize = 16;
pub const CHUNK_AREA: usize = CHUNK_WIDTH * CHUNK_WIDTH;
pub const CHUNK_HEIGHT: usize = 256;
pub const CHUNK_VOLUME: usize = CHUNK_AREA * CHUNK_HEIGHT;
pub const CHUNK_SECTION_HEIGHT: usize = 1;
pub const CHUNK_SECTIONS_PER: usize = CHUNK_HEIGHT / CHUNK_SECTION_HEIGHT;
pub const SECTION_VOLUME: usize = CHUNK_AREA * CHUNK_SECTION_HEIGHT;

use crate::WmRenderer;

pub type ChunkPos = (i32, i32);

#[derive(Clone, Debug)]
pub struct ChunkSection {
    pub empty: bool,
    pub blocks: Box<[BlockState; SECTION_VOLUME]>,
    pub offset_y: usize,
}

pub struct RenderLayers {
    terrain: Box<[ChunkSection; CHUNK_SECTIONS_PER]>,
    transparent: Box<[ChunkSection; CHUNK_SECTIONS_PER]>,
    grass: Box<[ChunkSection; CHUNK_SECTIONS_PER]>,
}

#[derive(Debug)]
pub struct ChunkLayers {
    grass: BakedChunkLayer<GrassVertex>,
    glass: BakedChunkLayer<TerrainVertex>,
    terrain: BakedChunkLayer<TerrainVertex>,
}

#[derive(Debug)]
pub struct Chunk {
    pub pos: ChunkPos,
    pub sections: Box<[ChunkSection; CHUNK_SECTIONS_PER]>,
    pub baked: ArcSwap<Option<ChunkLayers>>,
}

impl Chunk {
    #[must_use]
    pub fn new(pos: ChunkPos, blocks: Box<[BlockState; CHUNK_AREA * CHUNK_HEIGHT]>) -> Self {
        let sections: Box<[ChunkSection; CHUNK_SECTIONS_PER]> = (0..CHUNK_SECTIONS_PER)
            .map(|section| {
                let start_index = section * SECTION_VOLUME;
                let end_index = (section + 1) * SECTION_VOLUME;
                let block_section: Box<[BlockState; SECTION_VOLUME]> = (start_index..end_index)
                    .map(|index| blocks[index])
                    .collect::<Box<[BlockState]>>()
                    .try_into()
                    .unwrap();

                ChunkSection {
                    empty: !blocks.iter().any(|state| state.packed_key.is_some()),
                    blocks: block_section,
                    offset_y: section * CHUNK_SECTION_HEIGHT,
                }
            })
            .collect::<Box<[ChunkSection]>>()
            .try_into()
            .unwrap();

        Self {
            pos,
            sections,
            baked: ArcSwap::new(Arc::new(None)),
        }
    }

    #[must_use]
    pub fn blockstate_at_pos(&self, pos: BlockPos) -> BlockState {
        let x = (pos.0 % 16) as usize;
        let y = (pos.1) as usize;
        let z = (pos.2 % 16) as usize;

        self.sections[y].blocks[(z * CHUNK_WIDTH) + x]
    }

    pub fn bake(&self, block_manager: &BlockManager) {
        let grass_index = *block_manager
            .variant_indices
            .get(&"minecraft:blockstates/grass.json#".try_into().unwrap())
            .unwrap() as u32;

        let glass_index = *block_manager
            .variant_indices
            .get(&"minecraft:blockstates/glass.json#".try_into().unwrap())
            .unwrap() as u32;

        let grass = BakedChunkLayer::bake(
            block_manager,
            self,
            |v, x, y, z| GrassVertex {
                position: [v.position[0] + x, v.position[1] + y, v.position[2] + z],
                tex_coords: v.tex_coords,
                lightmap_coords: [0.0, 0.0],
                normal: v.normal,
                biome_color_coords: [0.0, 0.0],
            },
            Box::new(move |state| match state.packed_key {
                None => false,
                Some(key) => key == grass_index,
            }),
        );

        let glass = BakedChunkLayer::bake(
            block_manager,
            self,
            |v, x, y, z| TerrainVertex {
                position: [v.position[0] + x, v.position[1] + y, v.position[2] + z],
                tex_coords: v.tex_coords,
                lightmap_coords: [0.0, 0.0],
                normal: v.normal,
            },
            Box::new(move |state| match state.packed_key {
                None => false,
                Some(key) => key == glass_index,
            }),
        );

        let terrain = BakedChunkLayer::bake(
            block_manager,
            self,
            |v, x, y, z| TerrainVertex {
                position: [v.position[0] + x, v.position[1] + y, v.position[2] + z],
                tex_coords: v.tex_coords,
                lightmap_coords: [0.0, 0.0],
                normal: v.normal,
            },
            Box::new(move |state| match state.packed_key {
                None => false,
                Some(key) => key != grass_index && key != glass_index,
            }),
        );

        self.baked.store(Arc::new(Some(ChunkLayers {
            grass,
            glass,
            terrain,
        })));
    }
}

pub struct WorldBuffers {
    pub top: (wgpu::Buffer, usize),
    pub bottom: (wgpu::Buffer, usize),
    pub north: (wgpu::Buffer, usize),
    pub south: (wgpu::Buffer, usize),
    pub west: (wgpu::Buffer, usize),
    pub east: (wgpu::Buffer, usize),
    pub other: (wgpu::Buffer, usize),
}

pub struct ChunkManager {
    //Due to floating point inaccuracy at large distances,
    //we need to keep the model coordinates as close to 0,0,0 as possible
    pub chunk_origin: ArcSwap<ChunkPos>,
    pub loaded_chunks: RwLock<HashMap<ChunkPos, ArcSwap<Chunk>>>,
    pub section_buffers: ArcSwap<HashMap<String, WorldBuffers>>,
}

impl ChunkManager {
    #[must_use]
    pub fn new() -> Self {
        ChunkManager {
            chunk_origin: ArcSwap::new(Arc::new((0, 0))),
            loaded_chunks: RwLock::new(HashMap::new()),
            section_buffers: ArcSwap::new(Arc::new(HashMap::new())),
        }
    }

    pub fn bake_meshes(&self, wm: &WmRenderer) {
        let block_manager = wm.mc.block_manager.read();

        let chunks = {
            self.loaded_chunks
                .read()
                .iter()
                .map(|(_pos, chunk)| chunk.load_full())
                .collect::<Vec<_>>()
        };

        use rayon::iter::ParallelIterator;
        let time = Instant::now();
        chunks
            .par_iter()
            .for_each(|chunk| chunk.bake(&block_manager));
        println!(
            "Baked chunk in {}ms",
            Instant::now().duration_since(time).as_millis()
        );

        let mut glass = BakedChunkLayer::new();
        let mut grass = BakedChunkLayer::new();
        let mut terrain = BakedChunkLayer::new();

        chunks.iter().for_each(|chunk| {
            let baked = chunk.baked.load();
            let layers = (**baked).as_ref().unwrap();

            glass.extend(&layers.glass);
            grass.extend(&layers.grass);
            terrain.extend(&layers.terrain);
        });

        let mut map = HashMap::new();

        map.insert("transparent".into(), glass.upload(wm));
        map.insert("grass".into(), grass.upload(wm));
        map.insert("terrain".into(), terrain.upload(wm));

        self.section_buffers.store(Arc::new(map));
    }
}

impl Default for ChunkManager {
    fn default() -> Self {
        Self::new()
    }
}
