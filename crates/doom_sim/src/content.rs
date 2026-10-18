//! Content-derived tables the playsim needs at startup (`P_Init`):
//! texture/flat numbering for animations and switches, and texture heights
//! (used by "raise floor to shortest texture").

use doom_content::texture::TextureSet;
use doom_content::{LumpName, Namespace, ResourceStack};

#[derive(Clone, Debug, Default)]
pub struct ContentInfo {
    /// Texture names in texture-number order.
    pub texture_names: Vec<LumpName>,
    /// `textureheight[]`: height << FRACBITS.
    pub texture_heights: Vec<i32>,
    /// Flat names in flat-number order (merged flat namespace).
    pub flat_names: Vec<LumpName>,
}

impl ContentInfo {
    pub fn from_content(stack: &ResourceStack, textures: &TextureSet) -> Self {
        Self {
            texture_names: textures.textures.iter().map(|t| t.name).collect(),
            texture_heights: textures.textures.iter().map(|t| t.height << 16).collect(),
            flat_names: stack.namespace(Namespace::Flats).iter().map(|&id| stack.info(id).name).collect(),
        }
    }

    /// `R_CheckTextureNumForName`: first match; names starting with `-` are 0.
    pub fn texture_num(&self, name: &str) -> Option<usize> {
        let n = LumpName::new(name);
        if n.0[0] == b'-' {
            return Some(0);
        }
        self.texture_names.iter().position(|&t| t == n)
    }

    pub fn flat_num(&self, name: &str) -> Option<usize> {
        let n = LumpName::new(name);
        self.flat_names.iter().position(|&f| f == n)
    }
}
