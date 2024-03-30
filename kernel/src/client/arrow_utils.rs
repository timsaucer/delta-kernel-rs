//! Some utilities for working with arrow data types

use std::sync::Arc;

use crate::DeltaResult;

use arrow_array::RecordBatch;
use arrow_schema::{Schema as ArrowSchema, SchemaRef as ArrowSchemaRef};
use itertools::Itertools;
use parquet::{arrow::ProjectionMask, schema::types::SchemaDescriptor};

/// Get the indicies in `parquet_schema` of the specified columns in `requested_schema`. This
/// returns a tuples of (Vec<parquet_schema_index>, Vec<requested_index>). The
/// `parquet_schema_index` is used for generating the mask for reading from the parquet file, while
/// the requested_index is used for re-ordering. The requested_index vec will be -1 for any columns
/// that are not selected, and will contain at selected indexes the position that the column should
/// appear in the output
pub(crate) fn get_requested_indices(
    requested_schema: &ArrowSchema,
    parquet_schema: &ArrowSchemaRef,
) -> DeltaResult<(Vec<usize>, Vec<i32>)> {
    let mut reorder_indicies = vec![-1; parquet_schema.fields().len()];
    let indicies = requested_schema
        .fields
        .iter()
        .enumerate()
        .map(|(position, field)| {
            // todo: handle nested (then use `leaves` not `roots` below in generate_mask)
            let index = parquet_schema.index_of(field.name());
            if let Ok(index) = index {
                reorder_indicies[index] = position as i32;
            }
            index
        })
        .try_collect()?;
    Ok((indicies, reorder_indicies))
}

/// Create a mask that will only select the specified indicies from the parquet. Currently we only
/// handle "root" level columns, and hence use `ProjectionMask::roots`, but will support leaf
/// selection in the future. See issues #86 and #96 as well.
pub(crate) fn generate_mask(
    requested_schema: &ArrowSchema,
    parquet_schema: &ArrowSchemaRef,
    parquet_physical_schema: &SchemaDescriptor,
    indicies: &[usize],
) -> Option<ProjectionMask> {
    if parquet_schema.fields.size() == requested_schema.fields.size() {
        // we assume that in get_requested_indicies we will have caught any column name mismatches,
        // so here we can just say that if we request the same # of columns as the parquet file
        // actually has, we don't need to mask anything out
        None
    } else {
        Some(ProjectionMask::roots(
            parquet_physical_schema,
            indicies.to_owned(),
        ))
    }
}

/// Reorder a RecordBatch to match `requested_ordering`. This method takes `indicies` as computed by
/// [`get_requested_indicies`] as an optimization. If the indicies are in order, then we don't need
/// to do any re-ordering. Otherwise, for each non-zero value in `requested_ordering`, the column at
/// that index will be added in order to returned batch
pub(crate) fn reorder_record_batch(
    input_data: RecordBatch,
    indicies: &[usize],
    requested_ordering: &[i32],
) -> DeltaResult<RecordBatch> {
    if indicies.windows(2).all(|is| is[0] <= is[1]) {
        // indicies is already sorted, meaning we requested in the order that the columns were
        // stored in the parquet
        Ok(input_data)
    } else {
        // requested an order different from the parquet, reorder
        let input_schema = input_data.schema();
        let mut fields = Vec::with_capacity(indicies.len());
        let reordered_columns = requested_ordering
            .iter()
            .filter_map(|index| {
                if *index >= 0 {
                    let idx = *index as usize;
                    // cheap clones of `Arc`s
                    fields.push(input_schema.field(idx).clone());
                    Some(input_data.column(idx).clone())
                } else {
                    None
                }
            })
            .collect();
        let schema = Arc::new(ArrowSchema::new(fields));
        Ok(RecordBatch::try_new(schema, reordered_columns)?)
    }
}
