use channelbridge_core::{Config,Command,controller::Backend,worker::Worker};
use std::sync::{Arc,atomic::{AtomicUsize,Ordering},mpsc};
struct Fake(Arc<AtomicUsize>);
impl Backend for Fake {
 type Pending=(); type Session=();
 fn save(&mut self,_:&Config)->Result<(),String>{self.0.fetch_add(1,Ordering::SeqCst);Ok(())}
 fn negotiate(&mut self,_:&Config)->Result<(),String>{Ok(())}
 fn ready(&mut self,_:&())->Result<bool,String>{Ok(true)}
 fn open(&mut self,_:&Config)->Result<(),String>{Ok(())}
 fn healthy(&mut self,_:&(),_:&Config)->Result<bool,String>{Ok(true)}
 fn route(&mut self,_:&(),_:&Config,_:bool,_:bool)->Result<(),String>{Ok(())}
 fn close(&mut self,_:())->Result<(),String>{Ok(())}
}
#[test]
fn accepted_configuration_is_discarded_by_shutdown(){
 let count=Arc::new(AtomicUsize::new(0)); let saved=count.clone();
 let (release,gate)=mpsc::channel();
 let worker=Worker::spawn(Config::default(),move||{gate.recv().unwrap();Ok(Fake(saved))}).unwrap();
 worker.send(Command::Configure(Config{device_uid:"chosen".into(),..Config::default()})).unwrap();
 worker.request_shutdown();release.send(()).unwrap();worker.join().unwrap();
 assert_eq!(count.load(Ordering::SeqCst),1, "accepted configuration must be saved before shutdown completes");
}
#[test]
fn enabled_checkbox_config_is_rejected_without_device(){
 let mut controller=channelbridge_core::controller::Controller::new(Fake(Arc::new(AtomicUsize::new(0))),Config::default());
 let err=controller.command(Command::Configure(Config{auto_send_on_resume:true,..Config::default()})).unwrap_err();
 assert_eq!(err,"NoDevice"); assert!(!controller.config().auto_send_on_resume);
}
