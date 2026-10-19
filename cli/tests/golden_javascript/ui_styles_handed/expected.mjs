const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,8n];
const $k2=[20,$k1];
const $k3=[0,12n];
const $k4=[24,$k3];
const $k5=['p-8','p-8'];
const $k6=['w-12','w-12'];
const $k7=[$k5,$k6];
const $k8=[0,false];
const $k9=[2];
const $k10=[0n,'topstart'];
const $k11=[1n,'topend'];
const $k12=[2n,'bottomstart'];
const $k13=[3n,'bottomend'];
const $k14=[61n,0n,'vh','on'];
const $k15=[0n,0n,'',''];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n.p-8{padding:8px}\n.w-12{width:12px}\n';
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1],$k0[10],$k0[11]];
  const self_4=$host_HostStdout_println(ctx_1[1],'handed');
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  return $ui_node_mount(ctx_1,ui_node$stack$jvsnrs([[],[__cmd_x_main_buri$badge$jvsnrs($k2),__cmd_x_main_buri$badge$jvsnrs($k4)],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]),[]);
}
function __cmd_x_main_buri$badge$jvsnrs(look_0){
  return ui_node$stack$jvsnrs([[ui_style_classify$classifyOne$nc9115([$alloc_newCounter(-1n)],look_0,$k7,false,ui_style$extracted,-1n,-1n)],[],void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0]);
}
function ui_node$stack$jvsnrs(config_0){
  const events_1=[config_0[7],config_0[8],config_0[9],config_0[10],config_0[11],config_0[12],config_0[13],config_0[14]];
  let $t1;
  const $t2=config_0[3];
  if($t2!==void 0){
    $t1=$t2;
  }else if($t2===void 0){
    $t1=$k8;
  }else{
    $abort('no arm matched');
  }
  const current_2=$t1;
  const focus_3=[config_0[5],config_0[6]];
  const $t3=config_0[2];
  if($t3!==void 0){
    return [[4,$t3,$share(config_0[0]),$share(config_0[1]),current_2,focus_3,events_1]];
  }else if($t3===void 0){
    let $t4;
    const $t5=config_0[4];
    if($t5!==void 0){
      $t4=$t5;
    }else if($t5===void 0){
      $t4=false;
    }else{
      $abort('no arm matched');
    }
    const decorative_6=$t4;
    return [[3,$share(config_0[0]),$share(config_0[1]),current_2,decorative_6,focus_3,events_1]];
  }else{
    $abort('no arm matched');
  }
}
function ui_style$extracted(pairs_0){
  return [5,[pairs_0]];
}
function ui_style_classify$classifyOne$nc9115(ctx_0,style_1,known_2,strict_3,make_4,state_5,screen_6){
  const $t1=ui_style_classify$found$nc9115(ctx_0,[$share(style_1)],known_2,state_5,screen_6);
  switch($t1[0]){
    case 0:
      {
        return make_4($share($t1[1]));
      }
    case 1:
      {
        $ui_style_classify_unextracted(strict_3,$t1[1]);
        return ui_style_classify$within$nc9115(ctx_0,style_1,known_2,strict_3,make_4,state_5,screen_6);
      }
    case 2:
      {
        return ui_style_classify$within$nc9115(ctx_0,style_1,known_2,strict_3,make_4,state_5,screen_6);
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_style_classify$found$nc9115(ctx_0,styles_1,known_2,state_3,screen_4){
  const $t1=ui_style_classify$atomsIn$nc9115(ctx_0,styles_1,state_3,screen_4,[]);
  if($t1===void 0){
    return $k9;
  }else if($t1!==void 0){
    return ui_style_classify$named$nc9115(ctx_0,$list_foldCtx($t1,ctx_0,(c_6,out_7,a_8)=>{
      const $t2=$list_findIndex(out_7,o_12=>o_12[0]===a_8[0]);
      if($t2!==void 0){
        return $list_replaceAt(out_7,c_6,$t2,a_8);
      }else if($t2===void 0){
        return $list_push(out_7,c_6,a_8);
      }else{
        $abort('no arm matched');
      }
    },[]),known_2,0n,[]);
  }else{
    $abort('no arm matched');
  }
}
function ui_style_classify$within$nc9115(ctx_0,style_1,known_2,strict_3,make_4,state_5,screen_6){
  switch(style_1[0]){
    case 0:
      {
        const items_7=style_1[1];
        $share(items_7);
        return [0,ui_style_classify$classifyList$nc9115(ctx_0,items_7,known_2,strict_3,make_4,state_5,screen_6)];
      }
    case 1:
      {
        const on_8=style_1[1];
        const items_9=style_1[2];
        $share(items_9);
        return [0,ui_style_classify$classifyList$nc9115(ctx_0,items_9,known_2,strict_3,make_4,ui_style_classify$stateIndex(on_8),screen_6)];
      }
    case 2:
      {
        const at_10=style_1[1];
        const items_11=style_1[2];
        $share(items_11);
        let $t2;
        switch(at_10){
          case 0:
            {
              $t2=0n;
            }
            break;
          case 1:
            {
              $t2=1n;
            }
            break;
          case 2:
            {
              $t2=2n;
            }
            break;
          case 3:
            {
              $t2=3n;
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [0,ui_style_classify$classifyList$nc9115(ctx_0,items_11,known_2,strict_3,make_4,state_5,$t2)];
      }
    case 3:
      {
        const condition_12=style_1[1];
        const then_13=style_1[2];
        const otherwise_14=style_1[3];
        $share(then_13);
        $share(otherwise_14);
        return [3,condition_12,ui_style_classify$classifyList$nc9115(ctx_0,then_13,$share(known_2),strict_3,make_4,state_5,screen_6),ui_style_classify$classifyList$nc9115(ctx_0,otherwise_14,known_2,strict_3,make_4,state_5,screen_6)];
      }
    case 4:
      {
        const build_15=style_1[1];
        return [4,scope_16=>ui_style_classify$classify(build_15(scope_16),$share(known_2),strict_3,make_4)];
      }
    default:
      {
        return $share(style_1);
      }
  }
}
function ui_style_classify$classifyList$nc9115(ctx_0,styles_1,known_2,strict_3,make_4,state_5,screen_6){
  const $t1=ui_style_classify$found$nc9115(ctx_0,styles_1,known_2,state_5,screen_6);
  switch($t1[0]){
    case 0:
      {
        const pairs_7=$t1[1];
        const $t2=$list_length(pairs_7)===0n;
        if($t2){
          return [];
        }else if(!$t2){
          return [make_4($share(pairs_7))];
        }else{
          $abort('no arm matched');
        }
      }
      break;
    case 1:
      {
        $ui_style_classify_unextracted(strict_3,$t1[1]);
        return $list_mapCtx(styles_1,ctx_0,(c_9,s_10)=>ui_style_classify$classifyOne$nc9115(c_9,s_10,$share(known_2),strict_3,make_4,state_5,screen_6));
      }
    case 2:
      {
        return $list_mapCtx(styles_1,ctx_0,(c_11,s_12)=>ui_style_classify$classifyOne$nc9115(c_11,s_12,$share(known_2),strict_3,make_4,state_5,screen_6));
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_style_classify$stateIndex(state_0){
  switch(state_0){
    case 0:
      {
        return 0n;
      }
    case 1:
      {
        return 1n;
      }
    case 2:
      {
        return 2n;
      }
    case 3:
      {
        return 3n;
      }
    case 4:
      {
        return 4n;
      }
    case 5:
      {
        return 5n;
      }
    case 6:
      {
        return 6n;
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_style_classify$classify(styles_0,known_1,strict_2,make_3){
  return ui_style_classify$classifyList$nc9115([$alloc_newCounter(-1n)],styles_0,known_1,strict_2,make_3,-1n,-1n);
}
function ui_style_classify$atomsIn$nc9115(ctx_0,styles_1,state_2,screen_3,acc_4){
  return $list_foldCtx(styles_1,ctx_0,(c_5,sofar_6,style_7)=>{
    if(sofar_6!==void 0){
      $share(sofar_6);
      return ((inner_8,atoms_9)=>ui_style_classify$atomsOf$nc9115(inner_8,$share(style_7),state_2,screen_3,atoms_9))(c_5,sofar_6);
    }else if(sofar_6===void 0){
      return void 0;
    }else{
      $abort('no arm matched');
    }
  },acc_4);
}
function ui_style_classify$named$nc9115(ctx_0,atoms_1,known_2,at_3,acc_4){
  while(true){
    const $t1=$list_getFlat(atoms_1,at_3);
    if($t1===void 0){
      return [0,acc_4];
    }else if($t1!==void 0){
      const $t2=$list_findFlat(known_2,entry_6=>entry_6[0]===$t1[1]);
      if($t2===void 0){
        return [1,$t1[1]];
      }else if($t2!==void 0){
        at_3=at_3+1n;
        acc_4=$list_push(acc_4,ctx_0,[$t1[0],$t2[1]]);
        continue;
      }else{
        $abort('no arm matched');
      }
    }else{
      $abort('no arm matched');
    }
  }
}
function ui_style_classify$atomsOf$nc9115(ctx_0,style_1,state_2,screen_3,acc_4){
  switch(style_1[0]){
    case 0:
      {
        const items_5=style_1[1];
        $share(items_5);
        return ui_style_classify$atomsIn$nc9115(ctx_0,items_5,state_2,screen_3,acc_4);
      }
    case 1:
      {
        const on_6=style_1[1];
        const items_7=style_1[2];
        $share(items_7);
        return ui_style_classify$atomsIn$nc9115(ctx_0,items_7,ui_style_classify$stateIndex(on_6),screen_3,acc_4);
      }
    case 2:
      {
        const at_8=style_1[1];
        const items_9=style_1[2];
        $share(items_9);
        let $t2;
        switch(at_8){
          case 0:
            {
              $t2=0n;
            }
            break;
          case 1:
            {
              $t2=1n;
            }
            break;
          case 2:
            {
              $t2=2n;
            }
            break;
          case 3:
            {
              $t2=3n;
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return ui_style_classify$atomsIn$nc9115(ctx_0,items_9,state_2,$t2,acc_4);
      }
    case 3:
    case 4:
    case 5:
      {
        return void 0;
      }
    default:
      {
        $share(style_1);
        const $t4=ui_style_classify$declared$nc9115(ctx_0,style_1);
        const variant_11=$t4[0];
        const sub_12=$t4[1];
        const abbreviation_13=$t4[2];
        const key_14=$t4[3];
        const $t5=variant_11===15n?$list_push(acc_4,ctx_0,ui_style_classify$atom$nc9115(ctx_0,15n,4n,state_2,screen_3,'pin','out')):acc_4;
        return $list_push($t5,ctx_0,ui_style_classify$atom$nc9115(ctx_0,variant_11,sub_12,state_2,screen_3,abbreviation_13,key_14));
      }
  }
}
function ui_style_classify$declared$nc9115(ctx_0,style_1){
  switch(style_1[0]){
    case 6:
      {
        const layout_2=style_1[1];
        $share(layout_2);
        let $t2;
        switch(layout_2[0]){
          case 0:
            {
              $t2='col';
            }
            break;
          case 1:
            {
              $t2='row';
            }
            break;
          case 2:
            {
              $t2='colrev';
            }
            break;
          case 3:
            {
              $t2='rowrev';
            }
            break;
          case 4:
            {
              const tracks_71=layout_2[1];
              $share(tracks_71);
              const columns_74=$list_join($list_mapCtx(tracks_71,ctx_0,(c_72,track_73)=>{
                if(track_73[0]===0){
                  return $str_format(c_72,$str_fromInt(c_72,track_73[1])+'fr');
                }else if(track_73[0]===1){
                  return ui_style_classify$lengthCss$nc9115(c_72,track_73[1]);
                }else if(track_73[0]===2){
                  return 'auto';
                }else{
                  $abort('no arm matched');
                }
              }),ctx_0,' ');
              $t2=$str_format(ctx_0,'grid-'+$str_format(ctx_0,'#'+columns_74));
            }
            break;
          case 5:
            {
              $t2='layers';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [6n,0n,'lay',$t2];
      }
    case 7:
      {
        return [7n,0n,'am',ui_style_classify$alignKey(style_1[1])];
      }
    case 8:
      {
        let $t5;
        const $t6=style_1[1];
        $t5=$t6===0?'start':$t6===1?'center':$t6===2?'end':'stretch';
        return [8n,0n,'ac',$t5];
      }
    case 9:
      {
        let $t7;
        const $t8=style_1[1];
        $t7=$t8===0?'start':$t8===1?'center':$t8===2?'end':'stretch';
        return [9n,0n,'as',$t7];
      }
    case 10:
      {
        let $t9;
        const $t10=style_1[1];
        if($t10){
          $t9='wrap';
        }else if(!$t10){
          $t9='nowrap';
        }else{
          $abort('no arm matched');
        }
        return [10n,0n,'wrap',$t9];
      }
    case 11:
      {
        let $t11;
        const $t12=style_1[1];
        switch($t12){
          case 0:
            {
              $t11='x';
            }
            break;
          case 1:
            {
              $t11='y';
            }
            break;
          case 2:
            {
              $t11='both';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [11n,0n,'scroll',$t11];
      }
    case 12:
      {
        return [12n,0n,'grow',ui_style_classify$integerKey$nc9115(ctx_0,style_1[1])];
      }
    case 13:
      {
        return [13n,0n,'shrink',ui_style_classify$integerKey$nc9115(ctx_0,style_1[1])];
      }
    case 14:
      {
        return [14n,0n,'span',ui_style_classify$integerKey$nc9115(ctx_0,style_1[1])];
      }
    case 15:
      {
        const edge_11=style_1[1];
        const length_12=style_1[2];
        let $t13;
        switch(edge_11){
          case 0:
            {
              $t13=0n;
            }
            break;
          case 1:
            {
              $t13=1n;
            }
            break;
          case 2:
            {
              $t13=2n;
            }
            break;
          case 3:
            {
              $t13=3n;
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [15n,$t13,'pin',ui_style_classify$edgeKey$nc9115(ctx_0,edge_11,length_12)];
      }
    case 16:
      {
        let $t15;
        const $t16=style_1[1];
        switch($t16){
          case 0:
            {
              $t15='flow';
            }
            break;
          case 1:
            {
              $t15='sticky';
            }
            break;
          case 2:
            {
              $t15='viewport';
            }
            break;
          case 3:
            {
              $t15='anchor';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [16n,0n,'pos',$t15];
      }
    case 17:
      {
        return [17n,0n,'gap',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 18:
      {
        return [18n,0n,'gapx',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 19:
      {
        return [19n,0n,'gapy',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 20:
      {
        return [20n,0n,'p',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 21:
      {
        return [21n,0n,'px',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 22:
      {
        return [22n,0n,'py',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 23:
      {
        const edge_20=style_1[1];
        const length_21=style_1[2];
        let $t17;
        switch(edge_20){
          case 0:
            {
              $t17=0n;
            }
            break;
          case 1:
            {
              $t17=1n;
            }
            break;
          case 2:
            {
              $t17=2n;
            }
            break;
          case 3:
            {
              $t17=3n;
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [23n,$t17,'pe',ui_style_classify$edgeKey$nc9115(ctx_0,edge_20,length_21)];
      }
    case 24:
      {
        return [24n,0n,'w',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 25:
      {
        return [25n,0n,'h',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 26:
      {
        return [26n,0n,'minw',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 27:
      {
        return [27n,0n,'maxw',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 28:
      {
        return [28n,0n,'minh',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 29:
      {
        return [29n,0n,'maxh',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 30:
      {
        return [30n,0n,'ar',ui_style_classify$numberKey$nc9115(ctx_0,style_1[1])];
      }
    case 31:
      {
        return [31n,0n,'bg',ui_style_classify$colorKey$nc9115(ctx_0,style_1[1])];
      }
    case 32:
      {
        return [32n,0n,'fg',ui_style_classify$colorKey$nc9115(ctx_0,style_1[1])];
      }
    case 33:
      {
        return [33n,0n,'bw',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 34:
      {
        const edge_32=style_1[1];
        const length_33=style_1[2];
        let $t19;
        switch(edge_32){
          case 0:
            {
              $t19=0n;
            }
            break;
          case 1:
            {
              $t19=1n;
            }
            break;
          case 2:
            {
              $t19=2n;
            }
            break;
          case 3:
            {
              $t19=3n;
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [34n,$t19,'be',ui_style_classify$edgeKey$nc9115(ctx_0,edge_32,length_33)];
      }
    case 35:
      {
        return [35n,0n,'bc',ui_style_classify$colorKey$nc9115(ctx_0,style_1[1])];
      }
    case 36:
      {
        let $t21;
        const $t22=style_1[1];
        switch($t22){
          case 0:
            {
              $t21='none';
            }
            break;
          case 1:
            {
              $t21='solid';
            }
            break;
          case 2:
            {
              $t21='dashed';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [36n,0n,'bs',$t21];
      }
    case 37:
      {
        return [37n,0n,'r',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 38:
      {
        const length_38=style_1[2];
        let $t23;
        const $t24=style_1[1];
        switch($t24){
          case 0:
            {
              $t23=$k10;
            }
            break;
          case 1:
            {
              $t23=$k11;
            }
            break;
          case 2:
            {
              $t23=$k12;
            }
            break;
          case 3:
            {
              $t23=$k13;
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        const index_39=$t23[0];
        const name_40=$t23[1];
        return [38n,index_39,'rc',$str_format(ctx_0,name_40+'-'+ui_style_classify$lengthKey$nc9115(ctx_0,length_38))];
      }
    case 39:
      {
        return [39n,0n,'op',ui_style_classify$numberKey$nc9115(ctx_0,style_1[1])];
      }
    case 40:
      {
        const text_79=ui_style_classify$shadowCss$nc9115(ctx_0,style_1[1]);
        return [40n,0n,'sh',$str_format(ctx_0,'#'+text_79)];
      }
    case 41:
      {
        const layers_43=style_1[1];
        $share(layers_43);
        const text_81=$list_join($list_mapCtx(layers_43,ctx_0,(c_44,layer_45)=>ui_style_classify$shadowCss$nc9115(c_44,layer_45)),ctx_0,',');
        return [41n,0n,'sh',$str_format(ctx_0,'#'+text_81)];
      }
    case 42:
      {
        let $t25;
        const $t26=style_1[1];
        switch($t26[0]){
          case 0:
            {
              $t25='ui-sans-serif,system-ui,sans-serif';
            }
            break;
          case 1:
            {
              $t25='ui-serif,Georgia,serif';
            }
            break;
          case 2:
            {
              $t25='ui-monospace,SFMono-Regular,monospace';
            }
            break;
          case 3:
            {
              $t25=$str_format(ctx_0,'"'+$t26[1]+'",ui-sans-serif,sans-serif');
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        const text_83=$t25;
        return [42n,0n,'ff',$str_format(ctx_0,'#'+text_83)];
      }
    case 43:
      {
        return [43n,0n,'fs',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 44:
      {
        let $t27;
        const $t28=style_1[1];
        switch($t28){
          case 0:
            {
              $t27='regular';
            }
            break;
          case 1:
            {
              $t27='medium';
            }
            break;
          case 2:
            {
              $t27='semibold';
            }
            break;
          case 3:
            {
              $t27='bold';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [44n,0n,'fw',$t27];
      }
    case 45:
      {
        let $t29;
        const $t30=style_1[1];
        if($t30){
          $t29='italic';
        }else if(!$t30){
          $t29='normal';
        }else{
          $abort('no arm matched');
        }
        return [45n,0n,'it',$t29];
      }
    case 46:
      {
        return [46n,0n,'lh',ui_style_classify$numberKey$nc9115(ctx_0,style_1[1])];
      }
    case 47:
      {
        return [47n,0n,'ls',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 48:
      {
        return [48n,0n,'ta',ui_style_classify$alignKey(style_1[1])];
      }
    case 49:
      {
        let $t31;
        const $t32=style_1[1];
        switch($t32){
          case 0:
            {
              $t31='aswritten';
            }
            break;
          case 1:
            {
              $t31='upper';
            }
            break;
          case 2:
            {
              $t31='lower';
            }
            break;
          case 3:
            {
              $t31='capitalize';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [49n,0n,'tc',$t31];
      }
    case 50:
      {
        let $t33;
        const $t34=style_1[1];
        switch($t34){
          case 0:
            {
              $t33='none';
            }
            break;
          case 1:
            {
              $t33='underline';
            }
            break;
          case 2:
            {
              $t33='strike';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [50n,0n,'tl',$t33];
      }
    case 51:
      {
        let $t35;
        const $t36=style_1[1];
        switch($t36){
          case 0:
            {
              $t35='wrap';
            }
            break;
          case 1:
            {
              $t35='nowrap';
            }
            break;
          case 2:
            {
              $t35='balance';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [51n,0n,'tw',$t35];
      }
    case 52:
      {
        const lines_57=style_1[1];
        let $t37;
        const $t38=lines_57>0n;
        if($t38){
          $t37=ui_style_classify$integerKey$nc9115(ctx_0,lines_57);
        }else if(!$t38){
          $t37='0';
        }else{
          $abort('no arm matched');
        }
        return [52n,0n,'trunc',$t37];
      }
    case 53:
      {
        let $t39;
        const $t40=style_1[1];
        switch($t40){
          case 0:
            {
              $t39='default';
            }
            break;
          case 1:
            {
              $t39='pointer';
            }
            break;
          case 2:
            {
              $t39='text';
            }
            break;
          case 3:
            {
              $t39='notallowed';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [53n,0n,'cur',$t39];
      }
    case 54:
      {
        let $t41;
        const $t42=style_1[1];
        switch($t42){
          case 0:
            {
              $t41='none';
            }
            break;
          case 1:
            {
              $t41='disc';
            }
            break;
          case 2:
            {
              $t41='decimal';
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [54n,0n,'lm',$t41];
      }
    case 55:
      {
        const edge_60=style_1[1];
        const length_61=style_1[2];
        let $t43;
        switch(edge_60){
          case 0:
            {
              $t43=0n;
            }
            break;
          case 1:
            {
              $t43=1n;
            }
            break;
          case 2:
            {
              $t43=2n;
            }
            break;
          case 3:
            {
              $t43=3n;
            }
            break;
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
        return [55n,$t43,'bleed',ui_style_classify$edgeKey$nc9115(ctx_0,edge_60,length_61)];
      }
    case 56:
      {
        const y_63=style_1[2];
        return [56n,0n,'tr',$str_format(ctx_0,ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])+'-'+ui_style_classify$lengthKey$nc9115(ctx_0,y_63))];
      }
    case 57:
      {
        let $t45;
        const $t46=style_1[1];
        if($t46){
          $t45='clip';
        }else if(!$t46){
          $t45='visible';
        }else{
          $abort('no arm matched');
        }
        return [57n,0n,'clip',$t45];
      }
    case 58:
      {
        let $t47;
        const $t48=style_1[1];
        if($t48){
          $t47='none';
        }else if(!$t48){
          $t47='auto';
        }else{
          $abort('no arm matched');
        }
        return [58n,0n,'pass',$t47];
      }
    case 59:
      {
        return [59n,0n,'bdblur',ui_style_classify$lengthKey$nc9115(ctx_0,style_1[1])];
      }
    case 60:
      {
        return [60n,0n,'caret',ui_style_classify$colorKey$nc9115(ctx_0,style_1[1])];
      }
    case 61:
      {
        return $k14;
      }
    case 62:
      {
        let $t49;
        const $t50=style_1[1];
        if($t50===0){
          $t49='pulse';
        }else if($t50===1){
          $t49='spin';
        }else{
          $abort('no arm matched');
        }
        return [62n,0n,'anim',$t49];
      }
    default:
      {
        return $k15;
      }
  }
}
function ui_style_classify$atom$nc9115(ctx_0,variant_1,sub_2,state_3,screen_4,abbreviation_5,key_6){
  return [((variant_1===41n?40n:variant_1)*5n+sub_2)*40n+(state_3+1n)*5n+screen_4+1n,$str_format(ctx_0,(screen_4===0n?'sm_':screen_4===1n?'md_':screen_4===2n?'lg_':screen_4===3n?'xl_':'')+(state_3===0n?'hover_':state_3===1n?'focus_':state_3===2n?'focuswithin_':state_3===3n?'active_':state_3===4n?'disabled_':state_3===5n?'checked_':state_3===6n?'invalid_':'')+abbreviation_5+'-'+key_6)];
}
function ui_style_classify$alignKey(align_0){
  switch(align_0){
    case 0:
      {
        return 'start';
      }
    case 1:
      {
        return 'center';
      }
    case 2:
      {
        return 'end';
      }
    case 3:
      {
        return 'stretch';
      }
    case 4:
      {
        return 'between';
      }
    case 5:
      {
        return 'around';
      }
    case 6:
      {
        return 'evenly';
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_style_classify$integerKey$nc9115(ctx_0,n_1){
  const $t1=n_1<0n;
  if($t1){
    return $str_format(ctx_0,'n'+$str_fromInt(ctx_0,0n-n_1));
  }else if(!$t1){
    return $str_fromInt(ctx_0,n_1);
  }else{
    $abort('no arm matched');
  }
}
function ui_style_classify$edgeKey$nc9115(ctx_0,edge_1,length_2){
  let $t1;
  switch(edge_1){
    case 0:
      {
        $t1='top';
      }
      break;
    case 1:
      {
        $t1='bottom';
      }
      break;
    case 2:
      {
        $t1='start';
      }
      break;
    case 3:
      {
        $t1='end';
      }
      break;
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
  const name_3=$t1;
  return $str_format(ctx_0,name_3+'-'+ui_style_classify$lengthKey$nc9115(ctx_0,length_2));
}
function ui_style_classify$lengthKey$nc9115(ctx_0,length_1){
  switch(length_1[0]){
    case 0:
      {
        return ui_style_classify$integerKey$nc9115(ctx_0,length_1[1]);
      }
    case 1:
      {
        return $str_format(ctx_0,'r'+ui_style_classify$numberKey$nc9115(ctx_0,length_1[1]));
      }
    case 2:
      {
        return $str_format(ctx_0,'e'+ui_style_classify$numberKey$nc9115(ctx_0,length_1[1]));
      }
    case 3:
      {
        return $str_format(ctx_0,'pc'+ui_style_classify$numberKey$nc9115(ctx_0,length_1[1]));
      }
    case 4:
      {
        return 'auto';
      }
    case 5:
      {
        return 'full';
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_style_classify$numberKey$nc9115(ctx_0,n_1){
  return $str_replace($str_replace(ui_style_classify$number$nc9115(ctx_0,n_1),ctx_0,'.','_'),ctx_0,'-','n');
}
function ui_style_classify$colorKey$nc9115(ctx_0,color_1){
  switch(color_1[0]){
    case 0:
      {
        const g_3=color_1[2];
        const b_4=color_1[3];
        return $str_format(ctx_0,ui_style_classify$hex$nc9115(ctx_0,color_1[1])+ui_style_classify$hex$nc9115(ctx_0,g_3)+ui_style_classify$hex$nc9115(ctx_0,b_4));
      }
    case 1:
      {
        const g_6=color_1[2];
        const b_7=color_1[3];
        const a_8=color_1[4];
        return $str_format(ctx_0,ui_style_classify$hex$nc9115(ctx_0,color_1[1])+ui_style_classify$hex$nc9115(ctx_0,g_6)+ui_style_classify$hex$nc9115(ctx_0,b_7)+'a'+ui_style_classify$numberKey$nc9115(ctx_0,a_8));
      }
    case 2:
      {
        return ui_style_classify$tokenKey$nc9115(ctx_0,color_1[1]);
      }
    case 3:
      {
        return 'none';
      }
    case 4:
      {
        return 'inherit';
      }
    case 5:
      {
        const a_11=color_1[2];
        return $str_format(ctx_0,ui_style_classify$tokenKey$nc9115(ctx_0,color_1[1])+'a'+ui_style_classify$numberKey$nc9115(ctx_0,a_11));
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_style_classify$shadowCss$nc9115(ctx_0,shadow_1){
  const offset_2=$str_format(ctx_0,ui_style_classify$lengthCss$nc9115(ctx_0,shadow_1[0])+' '+ui_style_classify$lengthCss$nc9115(ctx_0,shadow_1[1]));
  const size_3=$str_format(ctx_0,ui_style_classify$lengthCss$nc9115(ctx_0,shadow_1[2])+' '+ui_style_classify$lengthCss$nc9115(ctx_0,shadow_1[3]));
  let $t1;
  const $t2=shadow_1[4];
  switch($t2[0]){
    case 0:
      {
        $t1=$str_format(ctx_0,'rgb('+ui_style_classify$channels$nc9115(ctx_0,$t2[1],$t2[2],$t2[3])+')');
      }
      break;
    case 1:
      {
        const a_12=$t2[4];
        $t1=$str_format(ctx_0,'rgba('+ui_style_classify$channels$nc9115(ctx_0,$t2[1],$t2[2],$t2[3])+','+ui_style_classify$number$nc9115(ctx_0,a_12)+')');
      }
      break;
    case 2:
      {
        const token_13=$t2[1];
        $t1=$str_format(ctx_0,'var(--'+token_13[0]+'-'+token_13[1]+')');
      }
      break;
    case 3:
      {
        $t1='transparent';
      }
      break;
    case 4:
      {
        $t1='inherit';
      }
      break;
    case 5:
      {
        const token_14=$t2[1];
        const percent_16=ui_style_classify$number$nc9115(ctx_0,$t2[2]*100);
        $t1=$str_format(ctx_0,'color-mix(in srgb,var(--'+token_14[0]+'-'+token_14[1]+') '+percent_16+'%,transparent)');
      }
      break;
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
  return $str_format(ctx_0,offset_2+' '+size_3+' '+$t1);
}
function ui_style_classify$lengthCss$nc9115(ctx_0,length_1){
  switch(length_1[0]){
    case 0:
      {
        return $str_format(ctx_0,$str_fromInt(ctx_0,length_1[1])+'px');
      }
    case 1:
      {
        return $str_format(ctx_0,ui_style_classify$number$nc9115(ctx_0,length_1[1])+'rem');
      }
    case 2:
      {
        return $str_format(ctx_0,ui_style_classify$number$nc9115(ctx_0,length_1[1])+'em');
      }
    case 3:
      {
        return $str_format(ctx_0,ui_style_classify$number$nc9115(ctx_0,length_1[1])+'%');
      }
    case 4:
      {
        return 'auto';
      }
    case 5:
      {
        return '100%';
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function ui_style_classify$channels$nc9115(ctx_0,r_1,g_2,b_3){
  return $str_format(ctx_0,$str_fromInt(ctx_0,r_1)+','+$str_fromInt(ctx_0,g_2)+','+$str_fromInt(ctx_0,b_3));
}
function ui_style_classify$number$nc9115(ctx_0,n_1){
  const text_2=$str_fromFloat(ctx_0,n_1);
  const $t1=$str_endsWith(text_2,'.0');
  if($t1){
    return $str_slice(text_2,0n,$str_length(text_2)-2n);
  }else if(!$t1){
    return text_2;
  }else{
    $abort('no arm matched');
  }
}
function ui_style_classify$hex$nc9115(ctx_0,n_1){
  const digits_2='0123456789abcdef';
  if(n_1>=0n&&n_1<256n){
    const high_3=$str_slice(digits_2,n_1/16n,n_1/16n+1n);
    const low_4=$str_slice(digits_2,n_1%16n,n_1%16n+1n);
    return $str_format(ctx_0,high_3+low_4);
  }else{
    return '?';
  }
}
function ui_style_classify$tokenKey$nc9115(ctx_0,token_1){
  const $t1=ui_style_classify$plain$nc9115(ctx_0,token_1[0]);
  const $t2=[$t1,ui_style_classify$plain$nc9115(ctx_0,token_1[1])];
  if($t2[0]&&$t2[1]){
    return $str_format(ctx_0,'t_'+token_1[0]+'_'+token_1[1]);
  }else{
    const text_9=$str_format(ctx_0,token_1[0]+'\u0001'+token_1[1]);
    return $str_format(ctx_0,'t_'+$str_format(ctx_0,'#'+text_9));
  }
}
function ui_style_classify$plain$nc9115(ctx_0,text_1){
  const $t1=$str_length(text_1)===0n;
  if($t1){
    return false;
  }else if(!$t1){
    return $list_all($str_splitAny(text_1,ctx_0,'0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ'),piece_2=>$str_length(piece_2)===0n);
  }else{
    $abort('no arm matched');
  }
}
const $buri$program={main:async ()=>{
  const $r=await __cmd_x_main_buri$main();
  $host.flush();
  return void $crossThrow($r);
}};
const $buri$ui={signal:(v)=>$host_HostUi_signal(null,v),write:(id,v)=>{$host_HostUi_write(null,id,v)}};
var $buri$host;
const $buri$platform = await (async () => {
$buri$host = () => ({ HostLocation: HostLocation, IndexedDb: IndexedDb });
// web's entry adapter. It starts the page's `main` when the module loads,
// implements `HostLocation`, the address bar, over the browser's own
// `location` and `history`, and `IndexedDb`, under `HostStorage`, over
// `indexedDB`.
//
// A JavaScript host that is not a browser has neither, so each is asked for
// before it is used: the page still runs to its first paint under bun, at `/`,
// with nowhere to push an address.
const { main } = $buri$program;
const { signal, write } = $buri$ui;

// The signal holding the path, made on first ask so a page that never routes
// registers nothing, and written from `popstate`, which is what the browser
// fires when the reader goes back or forward.
let address;

function current() {
  if (typeof location === "undefined" || location === null) return "/";
  return location.pathname || "/";
}

function hasHistory() {
  return typeof history !== "undefined" && history !== null;
}

// `ui/web`'s `navigate` and `replace` call `push` or `replace` and then write
// the signal themselves, so the graph and the address bar move together
// however the address was reached.
const HostLocation = {
  path: (self) => {
    if (address === undefined) {
      address = signal(current());
      if (typeof addEventListener === "function") {
        addEventListener("popstate", () => write(address, current()));
      }
    }
    return address;
  },
  push: (self, path) => {
    if (hasHistory()) history.pushState({}, "", path);
  },
  replace: (self, path) => {
    if (hasHistory()) history.replaceState({}, "", path);
  },
};

// `IndexedDb`, the store behind `HostStorage`: one object store, `values`, in
// a database named `buri`. Every method answers a promise, which the program
// awaits, and a failure throws the browser's reason. A full quota throws
// exactly `QuotaExceededError`, which `platform.buri` tells apart from a refusal.
let opened;

function database() {
  opened ??= new Promise((resolve, reject) => {
    if (typeof indexedDB === "undefined" || indexedDB === null) {
      throw new Error("this browser has no IndexedDB");
    }
    const request = indexedDB.open("buri", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("values");
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return opened;
}

function reason(e) {
  if (e && e.name === "QuotaExceededError") return new Error("QuotaExceededError");
  return new Error((e && (e.message || e.name)) || String(e));
}

// One request in a transaction of its own, answering its result once the
// transaction commits: a quota is only enforced at the commit.
async function transact(mode, act) {
  try {
    const db = await database();
    return await new Promise((resolve, reject) => {
      const tx = db.transaction("values", mode);
      const request = act(tx.objectStore("values"));
      tx.oncomplete = () => resolve(request.result);
      tx.onabort = () => reject(request.error ?? tx.error);
    });
  } catch (e) {
    throw reason(e);
  }
}

// The keys starting with `prefix`: from `prefix` up to the first string past
// every one of them, or every key for an empty prefix.
function startingWith(prefix) {
  for (let i = prefix.length - 1; i >= 0; i--) {
    const c = prefix.charCodeAt(i);
    if (c < 0xffff) {
      return IDBKeyRange.bound(prefix, prefix.slice(0, i) + String.fromCharCode(c + 1), false, true);
    }
  }
  return undefined;
}

const IndexedDb = {
  get: (self, key) => transact("readonly", (store) => store.get(key)),
  put: (self, key, value) => transact("readwrite", (store) => store.put(value, key)),
  delete: (self, key) => transact("readwrite", (store) => store.delete(key)),
  keys: (self, prefix) => transact("readonly", (store) => store.getAllKeys(startingWith(prefix))),
};

// `.Err(msg)` arrives as a thrown `Error`, and the message goes to the console.
// Under a host with a process, such as bun, the process exits 1.
await main().catch((e) => {
  console.error(e && e.message ? e.message : String(e));
  if (typeof process !== "undefined") process.exitCode = 1;
});
return { "HostLocation": HostLocation, "IndexedDb": IndexedDb };
})();
