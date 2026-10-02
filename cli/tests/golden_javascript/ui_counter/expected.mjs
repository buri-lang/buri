const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1200n,'lay-col'];
const $k2=[$k1];
const $k3=[$k2];
const $k4=[5,$k3];
const $k5=[$k4];
const $k6=[0,'doubled'];
const $k7=[0,false];
const $k8=[1200n,'lay-row'];
const $k9=[4200n,'px-r0_5'];
const $k10=[7400n,'r-6'];
const $k11=[6200n,'bg-f0f0f5'];
const $k12=[6400n,'fg-18181b'];
const $k13=[6205n,'hover_bg-18181b'];
const $k14=[6405n,'hover_fg-f0f0f5'];
const $k15=[$k8,$k9,$k10,$k11,$k12,$k13,$k14];
const $k16=[$k15];
const $k17=[5,$k16];
const $k18=[$k17];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n:where(h1,h2,h3,h4,h5,h6){font-size:inherit;font-weight:inherit;margin:0}\n:where(button){appearance:none;background:none;border:0;padding:0;font:inherit;color:inherit}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.px-r0_5{padding-inline:0.5rem}\n.bg-f0f0f5{background-color:rgb(240,240,245)}\n.hover_bg-18181b:hover{background-color:rgb(24,24,27)}\n.fg-18181b{color:rgb(24,24,27)}\n.hover_fg-f0f0f5:hover{color:rgb(240,240,245)}\n.r-6{border-radius:6px}\n';
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1],$k0[10],$k0[11]];
  const self_4=$host_HostStdout_println(ctx_1[1],'mounted');
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const label_8='clicks';
  const count_9=[$host_HostUi_signal(ctx_1[2],0n)];
  return $ui_node_mount(ctx_1,ui_node$stack$u3rqgv([$k5,[ui_node$button$u3rqgv([[0,label_8],[],void 0,c_10=>ui_signal$Signal_update$gb9o2y(count_9,c_10,n_11=>n_11+1n),void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]),__cmd_x_main_buri$badge$u3rqgv([0,label_8],[1,count_9]),__cmd_x_main_buri$badge$u3rqgv($k6,[2,c_12=>$ui_effect_Scope_read(c_12,count_9[0])*2n])],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]),[]);
}
function ui_signal$Signal_update$gb9o2y(self_0,ctx_1,f_2){
  return $host_HostUi_write(ctx_1[2],self_0[0],f_2($host_HostUi_read(ctx_1[2],self_0[0])));
}
function ui_node$button$u3rqgv(config_0){
  const events_1=ui_node$listen$u3rqgv(config_0[8],config_0[9],config_0[10],config_0[11],config_0[12],config_0[13],config_0[14],config_0[15]);
  const $t1=config_0[4];
  if($t1!==void 0&&$t1===1){
    const $t5=$share(config_0[1]);
    let $t2;
    const $t3=config_0[5];
    if($t3!==void 0){
      $t2=$t3;
    }else if($t3===void 0){
      $t2=$k7;
    }else{
      $abort('no arm matched');
    }
    return [[14,config_0[0],$t5,$t2,events_1]];
  }else{
    const $t9=$share(config_0[1]);
    const children_4=$share(config_0[2]);
    let $t6;
    if(children_4!==void 0){
      $t6=$share(children_4);
    }else if(children_4===void 0){
      $t6=[];
    }else{
      $abort('no arm matched');
    }
    let $t10;
    const $t11=config_0[3];
    if($t11!==void 0){
      $t10=(c_8,_event_9)=>$t11(c_8);
    }else if($t11===void 0){
      $t10=(_c_10,_event_11)=>0;
    }else{
      $abort('no arm matched');
    }
    let $t12;
    const $t13=config_0[5];
    if($t13!==void 0){
      $t12=$t13;
    }else if($t13===void 0){
      $t12=$k7;
    }else{
      $abort('no arm matched');
    }
    let $t14;
    const $t15=config_0[6];
    if($t15!==void 0){
      $t14=$t15;
    }else if($t15===void 0){
      $t14=$k7;
    }else{
      $abort('no arm matched');
    }
    let $t16;
    const $t17=config_0[7];
    if($t17!==void 0){
      $t16=$t17;
    }else if($t17===void 0){
      $t16=$k7;
    }else{
      $abort('no arm matched');
    }
    return [[5,config_0[0],$t9,$t6,$t10,$t12,$t14,$t16,events_1]];
  }
}
function __cmd_x_main_buri$badge$u3rqgv(title_0,count_1){
  return ui_node$stack$u3rqgv([$k18,[ui_node$text$u3rqgv([title_0,void 0,void 0]),ui_node$text$u3rqgv([[2,c_2=>{
    let $t1;
    if(count_1[0]===0){
      $t1=count_1[1];
    }else if(count_1[0]===1){
      $t1=$ui_effect_Scope_read(c_2,count_1[1][0]);
    }else if(count_1[0]===2){
      $t1=count_1[1](c_2);
    }else{
      $abort('no arm matched');
    }
    return String($t1);
  }],void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
}
function ui_node$stack$u3rqgv(config_0){
  const events_1=ui_node$listen$u3rqgv(config_0[3],config_0[4],config_0[5],config_0[6],config_0[7],config_0[8],config_0[9],config_0[10]);
  const $t1=config_0[2];
  if($t1!==void 0){
    return [[4,$t1,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else if($t1===void 0){
    return [[3,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else{
    $abort('no arm matched');
  }
}
function ui_node$listen$u3rqgv(onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4,onPointerDown_5,onPointerMove_6,onPointerUp_7){
  return [onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4,onPointerDown_5,onPointerMove_6,onPointerUp_7];
}
function ui_node$text$u3rqgv(config_0){
  const $t1=config_0[1];
  if($t1!==void 0){
    const styles_2=$share(config_0[2]);
    let $t2;
    if(styles_2!==void 0){
      $t2=$share(styles_2);
    }else if(styles_2===void 0){
      $t2=[];
    }else{
      $abort('no arm matched');
    }
    return [[2,$t1,$t2,config_0[0]]];
  }else if($t1===void 0){
    return [[1,config_0[0]]];
  }else{
    $abort('no arm matched');
  }
}
