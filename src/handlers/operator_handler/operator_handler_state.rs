use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::planner::{self, OperatorCompute};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Complete,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct OperatorInstance {
    pub id: u128,
    pub status: Status,
    pub query_id: u128,
    pub pipeline_id: String,
    pub operator: planner::Operator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotalOperatorCompute {
    pub instances: usize,
    pub memory_in_mib: usize,
    pub cpu_in_thousandths: usize,
}

impl TotalOperatorCompute {
    pub fn any_greater_than(&self, c: &TotalOperatorCompute) -> bool {
        if self.instances > c.instances {
            true
        } else if self.memory_in_mib > c.memory_in_mib {
            true
        } else if self.cpu_in_thousandths > c.cpu_in_thousandths {
            true
        } else {
            false
        }
    }
    pub fn add(&mut self, c: &TotalOperatorCompute) -> &Self {
        self.instances += c.instances;
        self.memory_in_mib += c.memory_in_mib;
        self.cpu_in_thousandths += c.cpu_in_thousandths;
        self
    }
    pub fn subtract(&mut self, c: &TotalOperatorCompute) -> &Self {
        self.instances = if self.instances > c.instances {
            self.instances - c.instances
        } else {
            0
        };
        self.memory_in_mib = if self.memory_in_mib > c.memory_in_mib {
            self.memory_in_mib - c.memory_in_mib
        } else {
            0
        };
        self.cpu_in_thousandths = if self.cpu_in_thousandths > c.cpu_in_thousandths {
            self.cpu_in_thousandths - c.cpu_in_thousandths
        } else {
            0
        };
        self
    }
    pub fn subtract_single_operator_compute(&mut self, c: &OperatorCompute) -> &Self {
        self.instances = if self.instances != 0 {
            self.instances - 1
        } else {
            0
        };
        self.memory_in_mib = if self.memory_in_mib > c.memory_in_mib {
            self.memory_in_mib - c.memory_in_mib
        } else {
            0
        };
        self.cpu_in_thousandths = if self.cpu_in_thousandths > c.cpu_in_thousandths {
            self.cpu_in_thousandths - c.cpu_in_thousandths
        } else {
            0
        };
        self
    }
    pub fn add_single_operator_compute(&mut self, c: &OperatorCompute) -> &Self {
        self.instances = c.instances;
        self.memory_in_mib = c.memory_in_mib;
        self.cpu_in_thousandths += c.cpu_in_thousandths;
        self
    }
    pub fn any_depleated(&self) -> bool {
        self.instances <= 0 || self.memory_in_mib <= 0 || self.cpu_in_thousandths <= 0
    }
}

pub struct OperatorHandlerState {
    operator_instances: Vec<OperatorInstance>,
    allowed_compute: TotalOperatorCompute,
}

impl OperatorHandlerState {
    pub fn new(allowed_compute: TotalOperatorCompute) -> OperatorHandlerState {
        OperatorHandlerState {
            operator_instances: Vec::new(),
            allowed_compute,
        }
    }

    pub fn add_operator_instance(&mut self, op_in: OperatorInstance) -> Result<()> {
        self.operator_instances.push(op_in);
        Ok(())
    }

    pub fn total_operator_compute(&self) -> TotalOperatorCompute {
        let mut total_compute = TotalOperatorCompute {
            instances: 0,
            memory_in_mib: 0,
            cpu_in_thousandths: 0,
        };
        for op in &self.operator_instances {
            let mut op_com = op.operator.compute.clone();
            op_com.instances = 1;
            total_compute.add_single_operator_compute(&op_com);
        }

        total_compute
    }

    pub fn get_allowed_compute(&self) -> TotalOperatorCompute {
        self.allowed_compute.clone()
    }
}
