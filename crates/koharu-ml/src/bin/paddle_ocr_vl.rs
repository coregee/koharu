use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use koharu_ml::paddle_ocr_vl::{PaddleOCRVL, PaddleOCRVLTask, PaddleOCRVLVariant};

#[derive(Debug, Parser)]
#[command(about = "Run PaddleOCR-VL element recognition")]
struct Cli {
    #[arg(short, long, value_name = "FILE")]
    input: PathBuf,

    #[arg(short, long, value_enum, default_value_t = Task::Ocr)]
    task: Task,

    #[arg(long, value_enum, default_value_t = Variant::General)]
    variant: Variant,

    #[arg(long, value_names = ["X", "Y", "WIDTH", "HEIGHT"], num_args = 4)]
    crop: Option<Vec<u32>>,

    #[arg(long, value_name = "DIRECTORY")]
    store: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    cpu: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Task {
    Ocr,
    Table,
    Formula,
    Chart,
    Spotting,
    Seal,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Variant {
    General,
    Manga,
}

impl From<Variant> for PaddleOCRVLVariant {
    fn from(value: Variant) -> Self {
        match value {
            Variant::General => Self::General,
            Variant::Manga => Self::Manga,
        }
    }
}

impl From<Task> for PaddleOCRVLTask {
    fn from(value: Task) -> Self {
        match value {
            Task::Ocr => Self::Ocr,
            Task::Table => Self::Table,
            Task::Formula => Self::Formula,
            Task::Chart => Self::Chart,
            Task::Spotting => Self::Spotting,
            Task::Seal => Self::Seal,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let cli = Cli::parse();
    if let Some(store) = cli.store {
        koharu_runtime::Store::configure(store)?;
    }
    let mut image = image::open(cli.input)?;
    if let Some(crop) = cli.crop {
        let [x, y, width, height] = <[u32; 4]>::try_from(crop)
            .map_err(|_| anyhow::anyhow!("crop requires x, y, width, and height"))?;
        let right = x.checked_add(width).context("crop width overflows")?;
        let bottom = y.checked_add(height).context("crop height overflows")?;
        anyhow::ensure!(
            right <= image.width() && bottom <= image.height(),
            "crop exceeds the source image"
        );
        image = image.crop_imm(x, y, width, height);
    }
    koharu_ml::init().await?;
    let device = koharu_ml::device(cli.cpu);
    let model = PaddleOCRVL::load(device, cli.variant.into()).await?;
    let result = model.inference(&image, cli.task.into())?;
    println!("{}", result.text);
    Ok(())
}
