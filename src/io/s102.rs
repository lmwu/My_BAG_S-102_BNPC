use crate::models::S102Point;
use hdf5;
use ndarray;

pub fn write_s102_hdf5_file(
    output_path: &str,
    points: &[S102Point],
    width: usize,
    height: usize,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    dx: f64,
    dy: f64,
    epsg: i32,
    vertical_datum: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = hdf5::File::create(output_path)?;

    file.new_attr::<hdf5::types::VarLenUnicode>()
        .create("productSpecification")?
        .write_scalar(&"INT.IHO.S-102.2.1".parse::<hdf5::types::VarLenUnicode>()?)?;
    file.new_attr::<hdf5::types::VarLenUnicode>()
        .create("issueDate")?
        .write_scalar(&"2026-09-03".parse::<hdf5::types::VarLenUnicode>()?)?;

    file.new_attr::<f64>()
        .create("westBoundLongitude")?
        .write_scalar(&min_x)?;
    file.new_attr::<f64>()
        .create("eastBoundLongitude")?
        .write_scalar(&max_x)?;
    file.new_attr::<f64>()
        .create("southBoundLatitude")?
        .write_scalar(&min_y)?;
    file.new_attr::<f64>()
        .create("northBoundLatitude")?
        .write_scalar(&max_y)?;

    file.new_attr::<hdf5::types::VarLenUnicode>()
        .create("horizontalDatumReference")?
        .write_scalar(&"EPSG".parse::<hdf5::types::VarLenUnicode>()?)?;
    file.new_attr::<i32>()
        .create("epsgCode")?
        .write_scalar(&epsg)?;
    file.new_attr::<i32>()
        .create("horizontalCRS")?
        .write_scalar(&epsg)?;

    file.new_attr::<i32>()
        .create("verticalDatum")?
        .write_scalar(&vertical_datum)?;

    // 2. 垂直坐標方向 (S-100 規範: 1 代表深度向下正值)
    file.new_attr::<i32>()
        .create("verticalCS")?
        .write_scalar(&1)?;

    // 3. 垂直坐標基準 (S-100 規範: 1 代表水面/海圖基準面 Surface)
    file.new_attr::<i32>()
        .create("verticalCoordinateBase")?
        .write_scalar(&1)?;

    let group_cov = file.create_group("BathymetryCoverage")?;
    let group_sub = group_cov.create_group("BathymetryCoverage.01")?;

    group_sub
        .new_attr::<f64>()
        .create("gridOriginLongitude")?
        .write_scalar(&min_x)?;
    group_sub
        .new_attr::<f64>()
        .create("gridOriginLatitude")?
        .write_scalar(&min_y)?;
    group_sub
        .new_attr::<f64>()
        .create("gridSpacingLongitudinal")?
        .write_scalar(&dx)?;
    group_sub
        .new_attr::<f64>()
        .create("gridSpacingLatitudinal")?
        .write_scalar(&dy)?;
    group_sub
        .new_attr::<u32>()
        .create("numPointsLongitudinal")?
        .write_scalar(&(width as u32))?;
    group_sub
        .new_attr::<u32>()
        .create("numPointsLatitudinal")?
        .write_scalar(&(height as u32))?;
    group_sub
        .new_attr::<hdf5::types::VarLenUnicode>()
        .create("startSequence")?
        .write_scalar(&"0,0".parse::<hdf5::types::VarLenUnicode>()?)?;

    let group_g001 = group_sub.create_group("Group_001")?;
    let points_2d = ndarray::ArrayView2::from_shape((height, width), points)?;
    let dataset = group_g001
        .new_dataset_builder()
        .empty::<S102Point>()
        .shape([height, width])
        .create("values")?;

    dataset.write(points_2d)?;

    Ok(())
}