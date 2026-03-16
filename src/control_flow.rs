use crate::steps::StepOutput;

/// Control flow exceptions (like Roast's skip!/break!/fail!)
#[derive(Debug)]
#[allow(dead_code)]
pub enum ControlFlow {
    /// Skip the current step without error
    Skip { message: String },

    /// Fail the step and potentially abort
    Fail { message: String },

    /// Exit the current repeat/map loop
    Break {
        message: String,
        value: Option<StepOutput>,
    },

    /// Skip to next iteration of repeat/map
    Next { message: String },
}
