use std::sync::Arc;

use anyhow::Result;
use arrow::{
    array::{BooleanArray, Float32Array, Int32Array, RecordBatch},
    datatypes::{DataType, Field, Schema},
};

use super::compute_value::compute_value;

#[test]
fn test_add() -> Result<()> {
    let id_array = Int32Array::from(vec![10, 20, 30, 40, 50]);
    let schema = Schema::new(vec![Field::new("cost", DataType::Int32, false)]);

    let rec = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(id_array)]).unwrap();
    let expr = sqlparser::ast::Expr::BinaryOp {
        op: sqlparser::ast::BinaryOperator::Plus,
        left: Box::new(sqlparser::ast::Expr::Identifier(sqlparser::ast::Ident {
            value: "cost".to_string(),
            quote_style: None,
        })),
        right: Box::new(sqlparser::ast::Expr::Value(sqlparser::ast::Value::Number(
            "7".to_string(),
            false,
        ))),
    };

    let res = compute_value(Arc::new(rec), Box::new(expr))?;
    let expected_res = Int32Array::from(vec![17, 27, 37, 47, 57]);

    assert!(res.get().0.eq(&expected_res));

    Ok(())
}

#[test]
fn test_eq() -> Result<()> {
    let id_array = Int32Array::from(vec![10, 20, 30, 40, 50]);
    let schema = Schema::new(vec![Field::new("cost", DataType::Int32, false)]);

    let rec = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(id_array)]).unwrap();
    let expr = sqlparser::ast::Expr::BinaryOp {
        op: sqlparser::ast::BinaryOperator::Eq,
        left: Box::new(sqlparser::ast::Expr::Identifier(sqlparser::ast::Ident {
            value: "cost".to_string(),
            quote_style: None,
        })),
        right: Box::new(sqlparser::ast::Expr::Value(sqlparser::ast::Value::Number(
            "20".to_string(),
            false,
        ))),
    };

    let res = compute_value(Arc::new(rec), Box::new(expr))?;
    let expected_res = BooleanArray::from(vec![false, true, false, false, false]);

    assert!(res.get().0.eq(&expected_res));

    Ok(())
}

#[test]
fn test_and_scalar_with_array() -> Result<()> {
    let id_array = BooleanArray::from(vec![true, false, true, false, true]);
    let schema = Schema::new(vec![Field::new("is_good", DataType::Boolean, false)]);

    let rec = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(id_array)]).unwrap();
    let expr = sqlparser::ast::Expr::BinaryOp {
        op: sqlparser::ast::BinaryOperator::Eq,
        left: Box::new(sqlparser::ast::Expr::Identifier(sqlparser::ast::Ident {
            value: "is_good".to_string(),
            quote_style: None,
        })),
        right: Box::new(sqlparser::ast::Expr::Value(sqlparser::ast::Value::Boolean(
            true,
        ))),
    };

    let res = compute_value(Arc::new(rec), Box::new(expr))?;
    let expected_res = BooleanArray::from(vec![true, false, true, false, true]);

    assert!(res.get().0.eq(&expected_res));

    Ok(())
}

#[test]
fn test_and_array_with_array() -> Result<()> {
    let tall_array = BooleanArray::from(vec![true, false, true, false, true]);
    let rich_array = BooleanArray::from(vec![false, true, true, true, false]);
    let schema = Schema::new(vec![
        Field::new("is_tall", DataType::Boolean, false),
        Field::new("is_rich", DataType::Boolean, false),
    ]);

    let rec = RecordBatch::try_new(
        Arc::new(schema),
        vec![Arc::new(tall_array), Arc::new(rich_array)],
    )
    .unwrap();
    let expr = sqlparser::ast::Expr::BinaryOp {
        op: sqlparser::ast::BinaryOperator::Eq,
        left: Box::new(sqlparser::ast::Expr::Identifier(sqlparser::ast::Ident {
            value: "is_tall".to_string(),
            quote_style: None,
        })),
        right: Box::new(sqlparser::ast::Expr::Identifier(sqlparser::ast::Ident {
            value: "is_rich".to_string(),
            quote_style: None,
        })),
    };

    let res = compute_value(Arc::new(rec), Box::new(expr))?;
    let expected_res = BooleanArray::from(vec![false, false, true, false, false]);

    assert!(res.get().0.eq(&expected_res));

    Ok(())
}

#[test]
fn test_complex_expression() -> Result<()> {
    // build the expression
    let dialect = sqlparser::dialect::GenericDialect {};
    let sql_text = "
    select * from abc
        where a+1.0/(2.0+c)*b
    ";
    let ast = match sqlparser::parser::Parser::parse_sql(&dialect, sql_text) {
        Ok(res) => res,
        Err(err) => {
            panic!("error: {err}");
        }
    };

    let stm = ast.get(0).unwrap();
    let query = match stm {
        sqlparser::ast::Statement::Query(query) => query,
        _ => panic!("expected statement to be a query"),
    };
    let select = match *query.body {
        sqlparser::ast::SetExpr::Select(ref select) => select,
        _ => panic!("expected select expression"),
    };
    let expr = select.clone().selection.expect("expression not set");

    // build the record
    let a = Float32Array::from(vec![0., 1., 2., 3.]);
    let b = Float32Array::from(vec![1., 1., 3., 2.]);
    let c = Float32Array::from(vec![0., 0., 1., 2.]);
    let schema = Schema::new(vec![
        Field::new("a", DataType::Float32, false),
        Field::new("b", DataType::Float32, false),
        Field::new("c", DataType::Float32, false),
    ]);

    let rec = RecordBatch::try_new(
        Arc::new(schema),
        vec![Arc::new(a), Arc::new(b), Arc::new(c)],
    )
    .unwrap();

    let res = compute_value(Arc::new(rec), Box::new(expr))?;
    let expected_res = Float32Array::from(vec![0.5, 1.5, 3.0, 3.5]);

    assert!(res.get().0.eq(&expected_res));

    Ok(())
}
