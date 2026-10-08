//! Machine-readable, read-only observation of one selected run. No planning or host startup.
use crate::cli::ObserveArgs;
use grida_fx_core::Error;
use grida_fx_runtime::observation;

pub fn run(args: &ObserveArgs) -> Result<u8, Error> {
    let root = super::planning::working_directory()?.join(&args.run);
    let result = if args.snapshot {
        observation::snapshot(&root)
            .map(|value| serde_json::to_value(value).expect("serializable snapshot"))
    } else {
        observation::batch(&root, args.after.as_deref(), args.limit)
            .map(|value| serde_json::to_value(value).expect("serializable event batch"))
    };
    match result {
        Ok(value) => {
            crate::print::print_json(&value);
            Ok(0)
        }
        Err(error) => {
            crate::print::print_json(
                &serde_json::to_value(error).expect("serializable observation error"),
            );
            Ok(2)
        }
    }
}
