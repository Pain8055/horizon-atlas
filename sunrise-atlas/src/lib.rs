//! Atlas World Map & Minimap for Sunrise / Starling Preview 4.
//! Pure `sunrise-mod-api` native mod (Compile v2 compatible: no mixins, no extra crates).

use mod_api::client::widgets::{Button, Outcome, Widgets};
use mod_api::prelude::*;
use mod_api::{
    ClientHost, ClientTickPostEvent, CommandEvent, Flow, GuiRenderPostEvent, Hooks, Identifier,
    InputKeyEvent, KeyAction, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseButtonReleasedPreEvent, ScreenMouseDraggedPreEvent, ScreenMouseScrolledPreEvent,
    ScreenRenderPostEvent, Text,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

const ID: &str = "atlas";
const MINIMAP_TEX: &str = "atlas:dynamic/minimap";
const WORLDMAP_TEX: &str = "atlas:dynamic/worldmap";

const MINI_SIZE: u32 = 128;
const WORLD_SIZE: u32 = 512;
const CHUNK_SCAN_BUDGET: usize = 2;

#[derive(Clone)]
struct Waypoint {
    name: String,
    x: i32,
    y: i32,
    z: i32,
    color: [u8; 4],
}

#[derive(Clone)]
struct ChunkTile {
    rgba: [u8; 16 * 16 * 4],
}

struct MapState {
    tiles: HashMap<(i32, i32), ChunkTile>,
    waypoints: Vec<Waypoint>,
    player_x: f64,
    player_y: f64,
    player_z: f64,
    player_yaw: f32,
    view_x: f64,
    view_z: f64,
    zoom: f64,
    minimap_enabled: bool,
    mini_created: bool,
    world_created: bool,
    world_dirty: bool,
    mini_rgba: Vec<u8>,
    world_rgba: Vec<u8>,
    dragging: bool,
    last_mouse: [f32; 2],
    tick_counter: u64,
    scan_ring_idx: usize,
}

impl Default for MapState {
    fn default() -> Self {
        Self {
            tiles: HashMap::new(),
            waypoints: Vec::new(),
            player_x: 0.0,
            player_y: 64.0,
            player_z: 0.0,
            player_yaw: 0.0,
            view_x: 0.0,
            view_z: 0.0,
            zoom: 1.0,
            minimap_enabled: true,
            mini_created: false,
            world_created: false,
            world_dirty: true,
            mini_rgba: vec![20u8; (MINI_SIZE * MINI_SIZE * 4) as usize],
            world_rgba: vec![16u8; (WORLD_SIZE * WORLD_SIZE * 4) as usize],
            dragging: false,
            last_mouse: [0.0, 0.0],
            tick_counter: 0,
            scan_ring_idx: 0,
        }
    }
}

struct MapScreenUi {
    widgets: Widgets,
    width: f32,
    height: f32,
    zoom_in: mod_api::client::widgets::WidgetId,
    zoom_out: mod_api::client::widgets::WidgetId,
    center: mod_api::client::widgets::WidgetId,
    add_wp: mod_api::client::widgets::WidgetId,
    close: mod_api::client::widgets::WidgetId,
}

impl MapScreenUi {
    fn new(width: f32, height: f32) -> Self {
        let mut widgets = Widgets::new();
        let y = (height - 28.0).max(8.0);
        let zoom_in = widgets.add_button(Button::new([8.0, y, 36.0, 22.0], Text::literal("+")));
        let zoom_out = widgets.add_button(Button::new([48.0, y, 36.0, 22.0], Text::literal("-")));
        let center =
            widgets.add_button(Button::new([88.0, y, 64.0, 22.0], Text::literal("Center")));
        let add_wp = widgets.add_button(Button::new([156.0, y, 56.0, 22.0], Text::literal("+WP")));
        let close = widgets.add_button(Button::new(
            [(width - 68.0).max(220.0), y, 60.0, 22.0],
            Text::literal("Close"),
        ));
        Self {
            widgets,
            width,
            height,
            zoom_in,
            zoom_out,
            center,
            add_wp,
            close,
        }
    }
}

#[derive(Default)]
struct AtlasMod {
    state: Mutex<MapState>,
    ui: Mutex<Option<MapScreenUi>>,
    open_req: AtomicBool,
    close_req: AtomicBool,
}

impl AtlasMod {
    fn scan_chunk(
        host: &dyn ClientHost,
        level: mod_api::LevelRef,
        cx: i32,
        cz: i32,
    ) -> Option<ChunkTile> {
        let states = host.block_states();
        let mut rgba = [0u8; 16 * 16 * 4];
        let mut found_any = false;
        let base_x = cx * 16;
        let base_z = cz * 16;

        for dz in 0..16i32 {
            for dx in 0..16i32 {
                let wx = base_x + dx;
                let wz = base_z + dz;
                let mut col = [24u8, 24, 32, 255];
                let mut top_y = 64i32;

                for y in (-64..=256i32).rev() {
                    if let Ok(bs) = level.block_state(host, [wx, y, wz]) {
                        if let Some(packed) = states.map_color(bs) {
                            if packed != 0 {
                                if let Some(c) = states.map_color_rgba(packed) {
                                    col = c;
                                    top_y = y;
                                    found_any = true;
                                    break;
                                }
                            }
                        }
                    }
                }

                let shade = ((top_y as f32 - 64.0) / 140.0).clamp(-0.22, 0.22);
                let mul = 1.0 + shade;
                let idx = ((dz as usize) * 16 + (dx as usize)) * 4;
                rgba[idx] = ((col[0] as f32) * mul).clamp(0.0, 255.0) as u8;
                rgba[idx + 1] = ((col[1] as f32) * mul).clamp(0.0, 255.0) as u8;
                rgba[idx + 2] = ((col[2] as f32) * mul).clamp(0.0, 255.0) as u8;
                rgba[idx + 3] = 255;
            }
        }

        if found_any {
            Some(ChunkTile { rgba })
        } else {
            None
        }
    }

    fn render_buffer(
        tiles: &HashMap<(i32, i32), ChunkTile>,
        waypoints: &[Waypoint],
        center_x: f64,
        center_z: f64,
        player_x: f64,
        player_z: f64,
        blocks_per_px: f64,
        size: u32,
        out: &mut [u8],
    ) {
        let half = (size as f64) * 0.5;
        for py in 0..size {
            let wz = (center_z + (py as f64 - half) * blocks_per_px).floor() as i32;
            let cz = wz.div_euclid(16);
            let lz = wz.rem_euclid(16) as usize;
            for px in 0..size {
                let wx = (center_x + (px as f64 - half) * blocks_per_px).floor() as i32;
                let cx = wx.div_euclid(16);
                let lx = wx.rem_euclid(16) as usize;
                let o = ((py * size + px) * 4) as usize;

                if let Some(tile) = tiles.get(&(cx, cz)) {
                    let t = (lz * 16 + lx) * 4;
                    out[o..o + 4].copy_from_slice(&tile.rgba[t..t + 4]);
                } else {
                    let grid = (((wx >> 4) + (wz >> 4)) & 1) == 0;
                    let v = if grid { 22 } else { 18 };
                    out[o] = v;
                    out[o + 1] = v;
                    out[o + 2] = v + 6;
                    out[o + 3] = 235;
                }
            }
        }

        for wp in waypoints {
            let sx = ((wp.x as f64 - center_x) / blocks_per_px + half).round() as i32;
            let sy = ((wp.z as f64 - center_z) / blocks_per_px + half).round() as i32;
            Self::draw_dot(out, size, sx, sy, 3, wp.color);
        }

        let psx = ((player_x - center_x) / blocks_per_px + half).round() as i32;
        let psy = ((player_z - center_z) / blocks_per_px + half).round() as i32;
        Self::draw_dot(out, size, psx, psy, 3, [255, 255, 255, 255]);
        Self::draw_dot(out, size, psx, psy, 1, [60, 210, 255, 255]);
    }

    fn draw_dot(buf: &mut [u8], size: u32, cx: i32, cy: i32, r: i32, color: [u8; 4]) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() + dy.abs() > r + 1 {
                    continue;
                }
                let x = cx + dx;
                let y = cy + dy;
                if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
                    let o = ((y as u32 * size + x as u32) * 4) as usize;
                    buf[o..o + 4].copy_from_slice(&color);
                }
            }
        }
    }
}

#[mod_api::sunrise_mod]
impl Hooks for AtlasMod {
    fn command(&self, _host: &mut dyn ClientHost, event: &mut CommandEvent<'_>) -> Flow {
        let raw = event.command.trim();
        if raw == "map" || raw == "atlas" {
            self.open_req.store(true, Ordering::Relaxed);
            return Flow::Cancel;
        }
        if let Some(rest) = raw.strip_prefix("wp ") {
            let name = rest.trim();
            if !name.is_empty() {
                let mut st = self.state.lock().unwrap();
                let wp = Waypoint {
                    name: name.to_owned(),
                    x: st.player_x.floor() as i32,
                    y: st.player_y.floor() as i32,
                    z: st.player_z.floor() as i32,
                    color: [255, 85, 85, 255],
                };
                st.waypoints.push(wp);
                st.world_dirty = true;
            }
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn input_key(&self, host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>) {
        if event.action == KeyAction::Press && event.key == "key.keyboard.m" && host.playing() {
            self.open_req.store(true, Ordering::Relaxed);
        }
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _: &mut ClientTickPostEvent) {
        if self.open_req.swap(false, Ordering::Relaxed) {
            {
                let mut st = self.state.lock().unwrap();
                st.view_x = st.player_x;
                st.view_z = st.player_z;
                st.world_dirty = true;
            }
            *self.ui.lock().unwrap() = None;
            host.mod_screen_open(ID);
        }
        if self.close_req.swap(false, Ordering::Relaxed) {
            host.mod_screen_close(ID);
        }

        if !host.playing() {
            return;
        }

        if let Some(cam) = host.camera() {
            let mut st = self.state.lock().unwrap();
            st.player_x = cam.position[0];
            st.player_y = cam.position[1];
            st.player_z = cam.position[2];
            st.player_yaw = cam.view_y_rot(1.0);
            st.tick_counter += 1;
        }

        let level = host
            .level("minecraft:overworld")
            .or_else(|| host.level("minecraft:the_nether"))
            .or_else(|| host.level("minecraft:the_end"));

        if let Some(level) = level {
            let (pcx, pcz, start_idx) = {
                let st = self.state.lock().unwrap();
                (
                    (st.player_x.floor() as i32).div_euclid(16),
                    (st.player_z.floor() as i32).div_euclid(16),
                    st.scan_ring_idx,
                )
            };

            let mut scanned = Vec::new();
            let mut idx = start_idx;
            for _ in 0..CHUNK_SCAN_BUDGET {
                let grid_i = idx % 49;
                let ox = (grid_i % 7) as i32 - 3;
                let oz = (grid_i / 7) as i32 - 3;
                let cx = pcx + ox;
                let cz = pcz + oz;
                if let Some(tile) = Self::scan_chunk(host, level, cx, cz) {
                    scanned.push(((cx, cz), tile));
                }
                idx = idx.wrapping_add(1);
            }

            let mut st = self.state.lock().unwrap();
            st.scan_ring_idx = idx;
            for (k, v) in scanned {
                st.tiles.insert(k, v);
                st.world_dirty = true;
            }
        }

        let upload_mini = {
            let mut st = self.state.lock().unwrap();
            if st.minimap_enabled && (st.tick_counter % 4 == 0 || !st.mini_created) {
                let px = st.player_x;
                let pz = st.player_z;
                let mut buf = std::mem::take(&mut st.mini_rgba);
                Self::render_buffer(
                    &st.tiles,
                    &st.waypoints,
                    px,
                    pz,
                    px,
                    pz,
                    1.0,
                    MINI_SIZE,
                    &mut buf,
                );
                st.mini_rgba = buf;
                Some((st.mini_created, st.mini_rgba.clone()))
            } else {
                None
            }
        };

        if let Some((created, pixels)) = upload_mini {
            let id = Identifier::parse(MINIMAP_TEX).unwrap();
            let res = if created {
                host.update_dynamic_texture(&id, MINI_SIZE, MINI_SIZE, &pixels)
            } else {
                host.create_dynamic_texture(&id, MINI_SIZE, MINI_SIZE, &pixels)
            };
            if res.is_ok() {
                self.state.lock().unwrap().mini_created = true;
            }
        }

        if host.mod_screen() == Some(ID) {
            let upload_world = {
                let mut st = self.state.lock().unwrap();
                if st.world_dirty || !st.world_created {
                    let vx = st.view_x;
                    let vz = st.view_z;
                    let px = st.player_x;
                    let pz = st.player_z;
                    let zoom = st.zoom;
                    let mut buf = std::mem::take(&mut st.world_rgba);
                    Self::render_buffer(
                        &st.tiles,
                        &st.waypoints,
                        vx,
                        vz,
                        px,
                        pz,
                        zoom,
                        WORLD_SIZE,
                        &mut buf,
                    );
                    st.world_rgba = buf;
                    st.world_dirty = false;
                    Some((st.world_created, st.world_rgba.clone()))
                } else {
                    None
                }
            };

            if let Some((created, pixels)) = upload_world {
                let id = Identifier::parse(WORLDMAP_TEX).unwrap();
                let res = if created {
                    host.update_dynamic_texture(&id, WORLD_SIZE, WORLD_SIZE, &pixels)
                } else {
                    host.create_dynamic_texture(&id, WORLD_SIZE, WORLD_SIZE, &pixels)
                };
                if res.is_ok() {
                    self.state.lock().unwrap().world_created = true;
                }
            }
        }
    }

    fn gui_render_post(&self, host: &dyn ClientHost, event: &mut GuiRenderPostEvent<'_>) {
        if !host.playing() || host.mod_screen() == Some(ID) {
            return;
        }
        let (enabled, created, px, py, pz) = {
            let st = self.state.lock().unwrap();
            (
                st.minimap_enabled,
                st.mini_created,
                st.player_x.floor() as i32,
                st.player_y.floor() as i32,
                st.player_z.floor() as i32,
            )
        };
        if !enabled || !created {
            return;
        }
        let [w, _] = event.batch.size();
        let draw_size = 96.0f32;
        let x = (w - draw_size - 8.0).max(4.0);
        let y = 8.0f32;
        event
            .batch
            .fill([x - 2.0, y - 2.0, draw_size + 4.0, draw_size + 4.0], 0xCC10_1018);
        event.batch.sprite(
            MINIMAP_TEX,
            [0.0, 0.0, MINI_SIZE as f32, MINI_SIZE as f32],
            [x, y, draw_size, draw_size],
            None,
        );
        let coords = format!("{px}, {py}, {pz}");
        event.batch.text(&coords, [x, y + draw_size + 4.0], 0xFFFF_FFFF, true);
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        if host.mod_screen() != Some(ID) {
            return;
        }
        let [width, height] = event.draw.size();
        event.draw.fill([0.0, 0.0, width, height], 0xEE0D_0F16);

        let world_created = self.state.lock().unwrap().world_created;
        if world_created {
            let map_size = (width.min(height - 36.0)).max(64.0);
            let mx = ((width - map_size) * 0.5).floor();
            let my = ((height - 32.0 - map_size) * 0.5).max(4.0).floor();
            event.draw.sprite(
                WORLDMAP_TEX,
                [0.0, 0.0, WORLD_SIZE as f32, WORLD_SIZE as f32],
                [mx, my, map_size, map_size],
                None,
            );
        }

        let mut ui = self.ui.lock().unwrap();
        let needs_rebuild = ui.as_ref().is_none_or(|u| {
            (u.width - width).abs() > 1.0 || (u.height - height).abs() > 1.0
        });
        if needs_rebuild {
            *ui = Some(MapScreenUi::new(width, height));
        }
        if let Some(ui) = ui.as_mut() {
            let _ = ui.widgets.draw(&mut *event.draw, host, event.screen.mouse);
        }
    }

    fn screen_mouse_button_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        let outcome = {
            let mut ui = self.ui.lock().unwrap();
            ui.as_mut().map(|u| {
                u.widgets
                    .mouse_pressed(host, event.screen.mouse, event.button, event.modifiers)
            })
        };
        if let Some(outcome) = outcome {
            let ui = self.ui.lock().unwrap();
            if let Some(u) = ui.as_ref() {
                match outcome {
                    Outcome::Pressed(id) if id == u.zoom_in => {
                        let mut st = self.state.lock().unwrap();
                        st.zoom = (st.zoom * 0.65).clamp(0.25, 8.0);
                        st.world_dirty = true;
                        return Flow::Cancel;
                    }
                    Outcome::Pressed(id) if id == u.zoom_out => {
                        let mut st = self.state.lock().unwrap();
                        st.zoom = (st.zoom * 1.5).clamp(0.25, 8.0);
                        st.world_dirty = true;
                        return Flow::Cancel;
                    }
                    Outcome::Pressed(id) if id == u.center => {
                        let mut st = self.state.lock().unwrap();
                        st.view_x = st.player_x;
                        st.view_z = st.player_z;
                        st.world_dirty = true;
                        return Flow::Cancel;
                    }
                    Outcome::Pressed(id) if id == u.add_wp => {
                        let mut st = self.state.lock().unwrap();
                        let n = st.waypoints.len() + 1;
                        let wp = Waypoint {
                            name: format!("WP {n}"),
                            x: st.player_x.floor() as i32,
                            y: st.player_y.floor() as i32,
                            z: st.player_z.floor() as i32,
                            color: [255, 85, 85, 255],
                        };
                        st.waypoints.push(wp);
                        st.world_dirty = true;
                        return Flow::Cancel;
                    }
                    Outcome::Pressed(id) if id == u.close => {
                        self.close_req.store(true, Ordering::Relaxed);
                        return Flow::Cancel;
                    }
                    _ if outcome.handled() => return Flow::Cancel,
                    _ => {}
                }
            }
        }

        let mut st = self.state.lock().unwrap();
        st.dragging = true;
        st.last_mouse = event.screen.mouse;
        Flow::Cancel
    }

    fn screen_mouse_button_released_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        self.state.lock().unwrap().dragging = false;
        if let Some(ui) = self.ui.lock().unwrap().as_mut() {
            let _ = ui.widgets.mouse_released(event.button);
        }
        Flow::Cancel
    }

    fn screen_mouse_dragged_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseDraggedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        let mut st = self.state.lock().unwrap();
        if st.dragging {
            let dx = event.screen.mouse[0] - st.last_mouse[0];
            let dy = event.screen.mouse[1] - st.last_mouse[1];
            st.last_mouse = event.screen.mouse;
            st.view_x -= (dx as f64) * st.zoom;
            st.view_z -= (dy as f64) * st.zoom;
            st.world_dirty = true;
        }
        Flow::Cancel
    }

    fn screen_mouse_scrolled_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        let mut st = self.state.lock().unwrap();
        if event.scroll[1] > 0.0 {
            st.zoom = (st.zoom * 0.8).clamp(0.25, 8.0);
        } else if event.scroll[1] < 0.0 {
            st.zoom = (st.zoom * 1.25).clamp(0.25, 8.0);
        }
        st.world_dirty = true;
        Flow::Cancel
    }

    fn screen_key_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        if event.key == "key.keyboard.escape" || event.key == "key.keyboard.m" {
            self.close_req.store(true, Ordering::Relaxed);
            return Flow::Cancel;
        }
        Flow::Continue
    }
}
