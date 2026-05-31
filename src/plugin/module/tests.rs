use std::{cell::RefCell, rc::Rc};

use super::Module;

struct TestModule {
    name: &'static str,
    log: Rc<RefCell<Vec<&'static str>>>,
    children: Vec<TestModule>,
}

impl Module for TestModule {
    fn free(&mut self) {
        self.log.borrow_mut().push(self.name);
    }

    fn children(&mut self) -> Vec<&mut dyn Module> {
        self.children
            .iter_mut()
            .map(|c| c as &mut dyn Module)
            .collect()
    }
}

fn leaf(name: &'static str, log: &Rc<RefCell<Vec<&'static str>>>) -> TestModule {
    TestModule {
        name,
        log: log.clone(),
        children: vec![],
    }
}

#[test]
fn handle_free_calls_children_in_reverse_then_self() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut root = TestModule {
        name: "root",
        log: log.clone(),
        children: vec![leaf("a", &log), leaf("b", &log)],
    };

    root.handle_free();

    assert_eq!(*log.borrow(), vec!["b", "a", "root"]);
}

#[test]
fn handle_free_recurses_into_grandchildren() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut root = TestModule {
        name: "root",
        log: log.clone(),
        children: vec![
            TestModule {
                name: "a",
                log: log.clone(),
                children: vec![leaf("a1", &log), leaf("a2", &log)],
            },
            leaf("b", &log),
        ],
    };

    root.handle_free();

    assert_eq!(*log.borrow(), vec!["b", "a2", "a1", "a", "root"]);
}
