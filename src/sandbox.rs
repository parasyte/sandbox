use crate::heap_array::{create_background_array, create_cells_array};
use crate::particle::{Particle, ParticleType};
use flume::{bounded as bounded_queue, Receiver};
use puffin::profile_scope;
use rand_pcg::Pcg64;
use simdnoise::NoiseBuilder;
use std::ops::{Index, IndexMut};
use std::thread;
use std::time::Instant;

pub const SANDBOX_WIDTH: usize = 480;
pub const SANDBOX_HEIGHT: usize = 270;

pub struct Sandbox {
    pub cells: Box<[[Option<Particle>; SANDBOX_HEIGHT]; SANDBOX_WIDTH]>,
    last_cells: Box<[[Option<Particle>; SANDBOX_HEIGHT]; SANDBOX_WIDTH]>,
    pub rng: Pcg64,
    update_counter: u8,
    background: Box<[u8; SANDBOX_HEIGHT * SANDBOX_WIDTH * 4]>,
    noise_queue_receiver: Receiver<Vec<f32>>,
}

impl Sandbox {
    pub fn new() -> Self {
        // Generate background
        let mut background = create_background_array(30);
        let mut i = 0;
        for y in 0..SANDBOX_HEIGHT {
            for x in 0..SANDBOX_WIDTH {
                // Generate grid
                if x % 8 == 0 || y % 8 == 0 {
                    background[i] = 60;
                    background[i + 1] = 60;
                    background[i + 2] = 60;
                }
                if x % 16 == 0 || y % 16 == 0 {
                    background[i] = 70;
                    background[i + 1] = 70;
                    background[i + 2] = 70;
                }
                if x % 32 == 0 || y % 32 == 0 {
                    background[i] = 80;
                    background[i + 1] = 80;
                    background[i + 2] = 80;
                }

                // Apply stripes
                if y % 2 == 0 {
                    background[i] -= 5;
                    background[i + 1] -= 5;
                    background[i + 2] -= 5;
                }

                // Apply vignette
                let x = x as isize - (SANDBOX_WIDTH as isize / 2);
                let y = y as isize - (SANDBOX_HEIGHT as isize / 2);
                let m = ((x.abs() + y.abs()) as f64 / 20.0).round() as u8;
                background[i] -= m;
                background[i + 1] -= m;
                background[i + 2] -= m;

                background[i + 3] = 255;
                i += 4;
            }
        }

        // Setup noise queue
        let (noise_queue_sender, noise_queue_receiver) = bounded_queue(10);
        thread::spawn(move || {
            let start_time = Instant::now();
            loop {
                let dt = start_time.elapsed().as_secs_f32() * 20.0;
                let noise = NoiseBuilder::turbulence_2d_offset(
                    dt,
                    SANDBOX_WIDTH * 2,
                    dt,
                    SANDBOX_HEIGHT / 2,
                )
                .generate_scaled(-1.0, 1.0);
                let _ = noise_queue_sender.send(noise);
            }
        });

        Self {
            cells: create_cells_array(None),
            last_cells: create_cells_array(None),
            rng: Pcg64::new(0xcafef00dd15ea5e5, 0xa02bdbf7bb3c0a7ac28fa16a64abf96),
            update_counter: 1,
            background,
            noise_queue_receiver,
        }
    }

    pub fn empty_out(&mut self) {
        for list in self.cells.iter_mut() {
            for cell in list.iter_mut() {
                *cell = None;
            }
        }
        self.update_counter = 1;
    }

    pub fn update(&mut self) {
        profile_scope!("update");

        {
            // Move particles
            profile_scope!("move_particles");
            self.update_counter = self.update_counter.checked_add(1).unwrap_or(1);
            for x in 0..SANDBOX_WIDTH {
                for y in 0..SANDBOX_HEIGHT {
                    if let Some(particle) = self[x][y] {
                        if particle.last_update != self.update_counter {
                            let new_particle_position = particle.move_particle(self, x, y);
                            self[new_particle_position.0][new_particle_position.1]
                                .as_mut()
                                .unwrap()
                                .last_update = self.update_counter
                        }
                    }
                }
            }
        }

        {
            // Transfer temperature between adjacent particles
            profile_scope!("temperature_transfer");
            self.last_cells.copy_from_slice(&self.cells[..]);
            for x in 0..SANDBOX_WIDTH {
                for y in 0..SANDBOX_HEIGHT {
                    if let Some(particle1) = &self.last_cells[x][y] {
                        let thermal_conductivity = particle1.thermal_conductivity();
                        let temperature = particle1.temperature;
                        if y != SANDBOX_HEIGHT - 1 {
                            if let Some(particle2) = &self[x][y + 1] {
                                let tc = thermal_conductivity + particle2.thermal_conductivity();
                                let t = temperature / tc;
                                self[x][y].as_mut().unwrap().temperature -= t;
                                self[x][y + 1].as_mut().unwrap().temperature += t;
                            }
                        }
                        if x != SANDBOX_WIDTH - 1 {
                            if let Some(particle2) = &self[x + 1][y] {
                                let tc = thermal_conductivity + particle2.thermal_conductivity();
                                let t = temperature / tc;
                                self[x][y].as_mut().unwrap().temperature -= t;
                                self[x + 1][y].as_mut().unwrap().temperature += t;
                            }
                        }
                        if y != 0 {
                            if let Some(particle2) = &self[x][y - 1] {
                                let tc = thermal_conductivity + particle2.thermal_conductivity();
                                let t = temperature / tc;
                                self[x][y].as_mut().unwrap().temperature -= t;
                                self[x][y - 1].as_mut().unwrap().temperature += t;
                            }
                        }
                        if x != 0 {
                            if let Some(particle2) = &self[x - 1][y] {
                                let tc = thermal_conductivity + particle2.thermal_conductivity();
                                let t = temperature / tc;
                                self[x][y].as_mut().unwrap().temperature -= t;
                                self[x - 1][y].as_mut().unwrap().temperature += t;
                            }
                        }
                    }
                }
            }
        }

        {
            // Perform particle interactions and state updates
            profile_scope!("update_particles");
            self.update_counter = self.update_counter.checked_add(1).unwrap_or(1);
            for x in 0..SANDBOX_WIDTH {
                for y in 0..SANDBOX_HEIGHT {
                    if let Some(particle) = self[x][y] {
                        if particle.last_update != self.update_counter {
                            particle.update(self, x, y);
                        }
                    }
                }
            }
        }
    }

    pub fn render(&mut self, frame: &mut [u8]) -> bool {
        profile_scope!("render_cpu");

        frame.copy_from_slice(&*self.background);

        let noise = self.noise_queue_receiver.recv().ok();

        let mut has_glow = false;
        let mut frame_index = 0;
        let mut noise_index = 0;
        for y in 0..SANDBOX_HEIGHT {
            for x in 0..SANDBOX_WIDTH {
                if let Some(particle) = &self.cells[x][y] {
                    // Base color
                    let base_color = particle.base_color(&mut self.rng);

                    // Tint blue/red based on temperature
                    let mut r = 0;
                    let mut b = 0;
                    let mut g = 0;
                    if particle.ptype != ParticleType::Electricity {
                        if particle.temperature < 0 {
                            b = -particle.temperature;
                            g = -particle.temperature / 30;
                        } else {
                            r = particle.temperature;
                        }
                    }

                    // Add Fire hue and shade
                    if particle.ptype == ParticleType::Fire {
                        g += particle.extra_data1 as i16;
                        r -= (particle.extra_data2 / 3) as i16;
                        g -= (particle.extra_data2 / 3) as i16;
                        b -= (particle.extra_data2 / 3) as i16;
                    }

                    // Darken/Lighten based on noise
                    let mut m = 0;
                    if let Some(noise) = &noise {
                        let shimmer_intensity = particle.shimmer_intensity();
                        if shimmer_intensity != 0 {
                            m = (noise[noise_index] * shimmer_intensity as f32) as i16;
                        }
                    }

                    // Combine everything together
                    let r = base_color.0 as i16 + r + m + particle.color_offset as i16;
                    let g = base_color.1 as i16 + g + m + particle.color_offset as i16;
                    let b = base_color.2 as i16 + b + m + particle.color_offset as i16;
                    let color = (
                        clamp(r, 0, 255) as u8,
                        clamp(g, 0, 255) as u8,
                        clamp(b, 0, 255) as u8,
                    );

                    frame[frame_index] = color.0;
                    frame[frame_index + 1] = color.1;
                    frame[frame_index + 2] = color.2;
                    frame[frame_index + 3] = if particle.is_glowing() {
                        has_glow = true;
                        0
                    } else {
                        255
                    };
                }

                frame_index += 4;
                noise_index += 1;
            }
        }

        has_glow
    }
}

impl Index<usize> for Sandbox {
    type Output = [Option<Particle>; SANDBOX_HEIGHT];

    fn index(&self, index: usize) -> &Self::Output {
        &self.cells[index]
    }
}

impl IndexMut<usize> for Sandbox {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.cells[index]
    }
}

fn clamp(value: i16, min: i16, max: i16) -> i16 {
    assert!(min <= max);
    let mut x = value;
    if x < min {
        x = min;
    }
    if x > max {
        x = max;
    }
    x
}
