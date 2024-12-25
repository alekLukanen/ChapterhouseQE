use anyhow::Result;
use thiserror::Error;
use uuid::Uuid;

use crate::{handlers::operator_handler::TotalOperatorCompute, planner};

#[derive(Debug, Clone, Error)]
pub enum QueryHandlerStateError {
    #[error("query not found: {0}")]
    QueryNotFound(u128),
    #[error("operator instance not found: {0}")]
    OperatorInstanceNotFound(u128),
    #[error("operator instance group not found for query_id {0} and instance {1}")]
    OperatorInstanceGroupNotFound(u128, u128),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Complete,
    Error(String),
}

impl Status {
    pub fn terminal(&self) -> bool {
        match self {
            Status::Complete => true,
            Status::Error(_) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OperatorInstance<'a> {
    pub id: u128,
    pub status: Status,
    pub operator: &'a planner::Operator,
}

impl<'a> OperatorInstance<'a> {
    pub fn new(op: &'a planner::Operator) -> OperatorInstance<'a> {
        OperatorInstance {
            id: Uuid::new_v4().as_u128(),
            status: Status::Queued,
            operator: op,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Query<'a> {
    pub id: u128,
    pub query: String,
    pub physical_plan: planner::PhysicalPlan,
    pub status: Status,

    pub operator_instances: Vec<OperatorInstance<'a>>,
}

impl<'a> Query<'a> {
    pub fn new(query: String, physical_plan: planner::PhysicalPlan) -> Query<'a> {
        Query {
            id: Uuid::new_v4().as_u128(),
            query,
            physical_plan,
            status: Status::Queued,
            operator_instances: Vec::new(),
        }
    }

    pub fn init(&'a mut self) -> &'a Self {
        self.add_operator_instances_from_physical_plan()
    }

    fn add_operator_instances_from_physical_plan(&'a mut self) -> &'a Self {
        for pipeline in self.physical_plan.get_pipelines_ref() {
            for op in pipeline.get_operators_ref() {
                self.operator_instances.push(OperatorInstance::new(op));
            }
        }
        self
    }
}

#[derive(Debug)]
pub struct QueryHandlerState<'a> {
    queries: Vec<Query<'a>>,
}

impl<'a> QueryHandlerState<'a> {
    pub fn new() -> QueryHandlerState<'a> {
        QueryHandlerState {
            queries: Vec::new(),
        }
    }

    pub fn add_query(&mut self, query: &'a Query) {
        self.queries.push(query.clone());
    }

    fn find_query(&'a self, query_id: u128) -> Result<&'a Query> {
        self.queries
            .iter()
            .find(|item| item.id == query_id)
            .ok_or(QueryHandlerStateError::QueryNotFound(query_id).into())
    }

    fn find_query_mut(&'a mut self, query_id: u128) -> Result<&'a mut Query> {
        self.queries
            .iter_mut()
            .find(|item| item.id == query_id)
            .ok_or(QueryHandlerStateError::QueryNotFound(query_id).into())
    }

    fn find_operator_instance(
        &'a self,
        query: &'a Query,
        op_instance_id: u128,
    ) -> Result<&'a OperatorInstance> {
        query
            .operator_instances
            .iter()
            .map(|item| {
                item.instances
                    .iter()
                    .filter(|inner_item| inner_item.id == op_instance_id)
                    .collect::<Vec<&OperatorInstance>>()
            })
            .flatten()
            .find(|item| item.id == op_instance_id.clone())
            .ok_or(QueryHandlerStateError::OperatorInstanceNotFound(op_instance_id).into())
    }

    fn find_operator_instance_group(
        &'a self,
        query: &'a Query,
        op_instance_id: u128,
    ) -> Result<&'a OperatorInstanceGroup> {
        query
            .operator_instances
            .iter()
            .find(|item| {
                item.instances
                    .iter()
                    .find(|inner_item| inner_item.id == op_instance_id)
                    .is_some()
            })
            .ok_or(
                QueryHandlerStateError::OperatorInstanceGroupNotFound(query.id, op_instance_id)
                    .into(),
            )
    }

    pub fn get_operator_instance(
        &self,
        query_id: u128,
        op_instance_id: u128,
    ) -> Result<OperatorInstance> {
        let query = self.find_query(query_id)?;
        let op_in = self.find_operator_instance(query, op_instance_id)?;
        Ok(op_in.clone())
    }

    pub fn get_available_operator_instance_ids(&self, query_id: u128) -> Result<Vec<u128>> {
        let query = self.find_query(query_id)?;
        Ok(query
            .operator_instances
            .iter()
            .map(|item| {
                item.instances
                    .iter()
                    .filter(|inner_item| inner_item.status == Status::Queued)
                    .map(|inner_item| inner_item.id)
                    .collect::<Vec<u128>>()
            })
            .flatten()
            .collect())
    }

    pub fn get_operator_instance_compute(
        &self,
        query_id: u128,
        op_instance_id: u128,
    ) -> Result<planner::OperatorCompute> {
        let query = self.find_query(query_id)?;
        let operator_group = query.operator_instances.iter().find(|item| {
            item.instances
                .iter()
                .find(|inner_item| inner_item.id == op_instance_id)
                .is_some()
        });
        match operator_group {
            Some(op_group) => Ok(op_group.operator.compute.clone()),
            None => Err(QueryHandlerStateError::OperatorInstanceNotFound(op_instance_id).into()),
        }
    }

    pub fn get_operator_instance_operator(
        &self,
        query_id: u128,
        op_instance_id: u128,
    ) -> Result<planner::Operator> {
        let query = self.find_query(query_id)?;
        let group = self.find_operator_instance_group(query, op_instance_id)?;
        Ok(group.operator.clone())
    }

    pub fn claim_operator_instances_up_to_compute_available(
        &mut self,
        available_compute: &TotalOperatorCompute,
    ) -> Vec<&OperatorInstance> {
        let mut compute = available_compute.clone();

        let mut result: Vec<&OperatorInstance> = Vec::new();
        for query in &mut self.queries {
            if compute.any_depleated() {
                break;
            }
            if query.status.terminal() {
                continue;
            }

            for group in &mut query.operator_instances {
                let group_compute = group.operator.compute.clone();
                if compute
                    .clone()
                    .subtract_single_operator_compute(&group_compute)
                    .any_depleated()
                {
                    continue;
                }

                for op_in in &mut group.instances {
                    if op_in.status == Status::Running || op_in.status.terminal() {
                        continue;
                    } else if compute
                        .clone()
                        .subtract_single_operator_compute(&group_compute.clone())
                        .any_depleated()
                    {
                        continue;
                    }

                    op_in.status = Status::Running;
                    compute.subtract_single_operator_compute(&group_compute);
                    result.push(op_in);
                }
            }
        }

        result
    }

    pub fn claim_operator_instance_if_queued(
        &mut self,
        query_id: u128,
        op_instance_id: u128,
    ) -> Result<bool> {
        let query = if let Some(query) = self.queries.iter_mut().find(|item| item.id == query_id) {
            query
        } else {
            return Err(QueryHandlerStateError::QueryNotFound(query_id).into());
        };

        let instance = if let Some(instance) = query
            .operator_instances
            .iter_mut()
            .map(|item| {
                item.instances
                    .iter_mut()
                    .filter(|inner_item| inner_item.id == op_instance_id)
                    .collect::<Vec<&mut OperatorInstance>>()
            })
            .flatten()
            .find(|item| item.id == op_instance_id.clone())
        {
            instance
        } else {
            return Err(QueryHandlerStateError::OperatorInstanceNotFound(op_instance_id).into());
        };

        if instance.status == Status::Queued {
            instance.status = Status::Running;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
