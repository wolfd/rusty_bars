extern crate libc;

use self::libc::{c_int};

/// Scales down a vector by averaging the elements between the resulting points
pub fn scale_fft_output(input: &[f64], new_len: usize) -> Vec<f64> {
    if new_len >= input.len() {
        return input.to_vec();
    }

    let band_size: usize = input.len() / new_len;
    assert!(band_size > 0);
    let mut output: Vec<f64> = Vec::with_capacity(new_len);

    let mut temp_count: usize = 0;
    let mut sum: f64 = 0.0;

    for &x in input.iter() {
        if temp_count >= band_size {
            let avg: f64 = sum/temp_count as f64;
            output.push(avg);
            temp_count = 0;
            sum = 0.0;
        } else {
            sum += x;
            temp_count+=1;
        }
    }

    if temp_count >= band_size {
        output.push(sum/temp_count as f64);
    }

    output
}


pub struct FFTDataOutput{
   // The number of fft columns to generate
   columns: usize
}


impl FFTDataOutput {
    /// Instantiate a new visualizer. Takes over the terminal with ncurses.
    pub fn new() -> FFTDataOutput {
        FFTDataOutput{
            columns: 10
        }
    }

    /// Render a single frame of the animation
    pub fn render_frame(&mut self, data: &[f64]) -> Result<(), c_int> {
        let data = scale_fft_output(data, self.columns as usize);
        println!("{:?}", data);
        Ok(())
    }
}


impl Drop for FFTDataOutput {
    fn drop(&mut self) {
    }
}


unsafe impl Send for FFTDataOutput {}
